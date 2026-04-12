.PHONY: build run test lint docker-build

build:
	go build -o bin/server ./cmd/server

run: build
	./bin/server

test:
	go test ./...

lint:
	golangci-lint run ./...

docker-build:
	docker build -t compute-backend .
