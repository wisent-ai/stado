# compute.wisent.com

GPU Compute Marketplace - a two-sided marketplace for renting GPU machines.

## Structure
- `backend/` - Go API server (chi router, pgxpool)
- `frontend/` - Next.js 14+ App Router (Supabase auth, shadcn/ui)
- `agent/` - Go host agent binary (runs on GPU machines)
- `supabase/` - Database migrations
- `deploy/` - Docker Compose files

## Backend
```bash
cd backend && go build -o server ./cmd/server && ./server
```

## Frontend
```bash
cd frontend && npm install && npm run dev
```

## Agent
```bash
cd agent && go build -o wisent-agent ./cmd/wisent-agent
```

## Database
Migrations are in `supabase/migrations/`. Apply with Supabase CLI.

## Key conventions
- Files must be ≤300 lines
- Max 5 files per directory
- No inline Python scripts
- No hardcoded timeouts
