.PHONY: build install test release

build:
	go build -o bin/wisent-agent ./cmd/wisent-agent

install: build
	sudo cp bin/wisent-agent /usr/local/bin/

test:
	go test ./...

release:
	GOOS=linux GOARCH=amd64 go build -o bin/wisent-agent-linux-amd64 ./cmd/wisent-agent
	GOOS=linux GOARCH=arm64 go build -o bin/wisent-agent-linux-arm64 ./cmd/wisent-agent
