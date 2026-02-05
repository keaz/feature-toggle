# Docker Setup for Feature Toggle System

This document describes how to build and run Docker images for the feature toggle system components.

## Architecture Overview

The system consists of three main components:
- **PostgreSQL Database**: Stores feature flags, user data, and configurations
- **Feature Toggle Backend**: Main API server (REST + gRPC) running on port 8080
- **Feature Edge Server**: Lightweight evaluation server running on port 8081

## Quick Start

### Using Docker Compose (Recommended)

Build and start all services:
```bash
docker-compose up --build
```

This will start:
- PostgreSQL database on port 5433
- Feature toggle backend on port 8080
- Feature edge server on port 8081

### Building Individual Images

#### Feature Toggle Backend
```bash
docker build -f feature-toggle-backend/Dockerfile -t feature-toggle-backend .
```

#### Feature Edge Server
```bash
docker build -f feature-edge-server/Dockerfile -t feature-edge-server .
```

## Running Individual Containers

### Prerequisites
- PostgreSQL database running and accessible
- Database migrations completed

### Feature Toggle Backend
```bash
docker run -d \
  --name feature-toggle-backend \
  -p 8080:8080 \
  -p 50051:50051 \
  -e DATABASE_URL=postgres://postgres:password@host:5432/feature_toggle \
  feature-toggle-backend
```

### Feature Edge Server
```bash
docker run -d \
  --name feature-edge-server \
  -p 8081:8081 \
  -e EDGE_BACKEND_GRPC=http://backend-host:50051 \
  -e EDGE_HTTP_ADDR=0.0.0.0:8081 \
  -e EDGE_CLIENT_ID=your-client-id \
  -e EDGE_CLIENT_SECRET=your-client-secret \
  feature-edge-server
```

## Environment Variables

### Feature Toggle Backend
- `DATABASE_URL`: PostgreSQL connection string
- Additional configuration via `config.toml`

### Feature Edge Server
- `EDGE_BACKEND_GRPC`: gRPC endpoint of the backend service
- `EDGE_HTTP_ADDR`: HTTP server bind address
- `EDGE_CLIENT_ID`: Client ID for authentication
- `EDGE_CLIENT_SECRET`: Client secret for authentication

## Development

### Local Development Setup
For local development, you may want to run the services outside Docker while using Docker for PostgreSQL:

```bash
# Start only PostgreSQL
docker-compose up postgres_server

# Run backend locally
cd feature-toggle-backend
cargo run

# Run edge server locally (in another terminal)
cd feature-edge-server
cargo run
```

### Logs
To view logs from the services:
```bash
docker-compose logs -f feature_toggle_backend
docker-compose logs -f feature_edge_server
```

## Production Considerations

1. **Security**: Update default passwords and secrets
2. **Persistence**: Use named volumes for PostgreSQL data
3. **Health Checks**: Consider adding health check endpoints
4. **Monitoring**: Add monitoring and logging solutions
5. **Scaling**: Consider horizontal scaling for edge servers

## Troubleshooting

### Common Issues

1. **Database Connection Issues**
   - Ensure PostgreSQL is running and accessible
   - Check DATABASE_URL format
   - Verify network connectivity between containers

2. **Migration Failures**
   - Check database permissions
   - Ensure init.sql is accessible
   - Verify sqlx-cli installation

3. **gRPC Connection Issues**
   - Verify backend is exposing port 50051
   - Check EDGE_BACKEND_GRPC configuration
   - Ensure network connectivity between services

### Debugging
To debug a specific service:
```bash
# Get shell access to running container
docker exec -it feature_toggle_backend bash

# View container logs
docker logs feature_toggle_backend

# Check container resource usage
docker stats feature_toggle_backend
```
