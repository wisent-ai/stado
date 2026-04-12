FROM golang:1.22-alpine AS builder
WORKDIR /app
COPY go.mod go.sum ./
RUN go mod download
COPY . .
RUN CGO_ENABLED=0 GOOS=linux go build -o /wisent-agent ./cmd/wisent-agent

FROM nvidia/cuda:12.4.0-base-ubuntu22.04
RUN apt-get update && apt-get install -y docker.io && rm -rf /var/lib/apt/lists/*
COPY --from=builder /wisent-agent /usr/local/bin/wisent-agent
ENTRYPOINT ["wisent-agent"]
