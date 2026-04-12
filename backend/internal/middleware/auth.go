package middleware

import (
	"context"
	"net/http"
	"strings"

	"github.com/golang-jwt/jwt/v5"
	"github.com/google/uuid"
	"github.com/jackc/pgx/v5/pgxpool"
	"github.com/rs/zerolog/log"
	"golang.org/x/crypto/bcrypt"
)

type contextKey string

const (
	UserIDKey contextKey = "user_id"
	UserRoleKey contextKey = "user_role"
)

func GetUserID(ctx context.Context) uuid.UUID {
	id, _ := ctx.Value(UserIDKey).(uuid.UUID)
	return id
}

func GetUserRole(ctx context.Context) string {
	role, _ := ctx.Value(UserRoleKey).(string)
	return role
}

func Auth(jwtSecret string, db *pgxpool.Pool) func(http.Handler) http.Handler {
	return func(next http.Handler) http.Handler {
		return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			auth := r.Header.Get("Authorization")
			if auth == "" {
				http.Error(w, "unauthorized", http.StatusUnauthorized)
				return
			}

			token := strings.TrimPrefix(auth, "Bearer ")

			// Check if it's an API key (prefixed with wsk_)
			if strings.HasPrefix(token, "wsk_") {
				userID, role, err := validateAPIKey(r.Context(), db, token)
				if err != nil {
					http.Error(w, "invalid api key", http.StatusUnauthorized)
					return
				}
				ctx := context.WithValue(r.Context(), UserIDKey, userID)
				ctx = context.WithValue(ctx, UserRoleKey, role)
				next.ServeHTTP(w, r.WithContext(ctx))
				return
			}

			// JWT verification
			claims := &jwt.RegisteredClaims{}
			_, err := jwt.ParseWithClaims(token, claims, func(t *jwt.Token) (interface{}, error) {
				return []byte(jwtSecret), nil
			})
			if err != nil {
				http.Error(w, "invalid token", http.StatusUnauthorized)
				return
			}

			userID, err := uuid.Parse(claims.Subject)
			if err != nil {
				http.Error(w, "invalid token subject", http.StatusUnauthorized)
				return
			}

			var role string
			err = db.QueryRow(r.Context(), "SELECT role FROM profiles WHERE id = $1", userID).Scan(&role)
			if err != nil {
				log.Warn().Err(err).Str("user_id", userID.String()).Msg("profile not found")
				role = "user"
			}

			ctx := context.WithValue(r.Context(), UserIDKey, userID)
			ctx = context.WithValue(ctx, UserRoleKey, role)
			next.ServeHTTP(w, r.WithContext(ctx))
		})
	}
}

func validateAPIKey(ctx context.Context, db *pgxpool.Pool, key string) (uuid.UUID, string, error) {
	prefix := key[:12]
	rows, err := db.Query(ctx,
		`SELECT ak.key_hash, ak.user_id, p.role FROM api_keys ak
		 JOIN profiles p ON p.id = ak.user_id
		 WHERE ak.key_prefix = $1 AND ak.revoked_at IS NULL`, prefix)
	if err != nil {
		return uuid.Nil, "", err
	}
	defer rows.Close()

	for rows.Next() {
		var hash string
		var userID uuid.UUID
		var role string
		if err := rows.Scan(&hash, &userID, &role); err != nil {
			continue
		}
		if bcrypt.CompareHashAndPassword([]byte(hash), []byte(key)) == nil {
			_, _ = db.Exec(ctx, "UPDATE api_keys SET last_used_at = now() WHERE key_prefix = $1", prefix)
			return userID, role, nil
		}
	}
	return uuid.Nil, "", http.ErrAbortHandler
}

func RequireHost(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		role := GetUserRole(r.Context())
		if role != "host" && role != "admin" {
			http.Error(w, "host access required", http.StatusForbidden)
			return
		}
		next.ServeHTTP(w, r)
	})
}

func RequireAdmin(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		role := GetUserRole(r.Context())
		if role != "admin" {
			http.Error(w, "admin access required", http.StatusForbidden)
			return
		}
		next.ServeHTTP(w, r)
	})
}
