package ws

import (
	"encoding/json"
	"sync"

	"github.com/google/uuid"
)

type Event struct {
	Type    string      `json:"type"`
	Payload interface{} `json:"payload"`
	UserID  uuid.UUID   `json:"-"`
}

type Hub struct {
	mu      sync.RWMutex
	clients map[uuid.UUID]map[*Client]bool
}

func NewHub() *Hub {
	return &Hub{
		clients: make(map[uuid.UUID]map[*Client]bool),
	}
}

func (h *Hub) Register(userID uuid.UUID, client *Client) {
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.clients[userID] == nil {
		h.clients[userID] = make(map[*Client]bool)
	}
	h.clients[userID][client] = true
}

func (h *Hub) Unregister(userID uuid.UUID, client *Client) {
	h.mu.Lock()
	defer h.mu.Unlock()
	if conns, ok := h.clients[userID]; ok {
		delete(conns, client)
		if len(conns) == 0 {
			delete(h.clients, userID)
		}
	}
}

func (h *Hub) SendToUser(userID uuid.UUID, event Event) {
	h.mu.RLock()
	defer h.mu.RUnlock()
	conns, ok := h.clients[userID]
	if !ok {
		return
	}
	data, err := json.Marshal(event)
	if err != nil {
		return
	}
	for client := range conns {
		select {
		case client.send <- data:
		default:
			close(client.send)
			delete(conns, client)
		}
	}
}

func (h *Hub) Broadcast(event Event) {
	h.mu.RLock()
	defer h.mu.RUnlock()
	data, err := json.Marshal(event)
	if err != nil {
		return
	}
	for _, conns := range h.clients {
		for client := range conns {
			select {
			case client.send <- data:
			default:
				close(client.send)
			}
		}
	}
}
