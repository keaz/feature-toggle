#!/bin/bash
set -e

echo "=== Feature Toggle Services Health Check ==="

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Function to check if a service is healthy
check_service() {
    local service_name=$1
    local url=$2
    local expected_status=${3:-200}
    
    echo -n "Checking $service_name... "
    
    if response=$(curl -s -o /dev/null -w "%{http_code}" "$url" 2>/dev/null); then
        if [ "$response" = "$expected_status" ]; then
            echo -e "${GREEN}✓ Healthy (HTTP $response)${NC}"
            return 0
        else
            echo -e "${RED}✗ Unhealthy (HTTP $response)${NC}"
            return 1
        fi
    else
        echo -e "${RED}✗ Unreachable${NC}"
        return 1
    fi
}

# Function to check database connection
check_database() {
    echo -n "Checking PostgreSQL... "
    
    if docker-compose exec -T postgres_server pg_isready -U postgres >/dev/null 2>&1; then
        echo -e "${GREEN}✓ Database is ready${NC}"
        return 0
    else
        echo -e "${RED}✗ Database is not ready${NC}"
        return 1
    fi
}

# Function to check docker-compose services
check_docker_services() {
    echo -e "\n${YELLOW}Docker Compose Services Status:${NC}"
    docker-compose ps
}

# Main health checks
echo "Health check started at $(date)"
echo ""

# Check if docker-compose is running
if ! docker-compose ps >/dev/null 2>&1; then
    echo -e "${RED}✗ Docker Compose services are not running${NC}"
    echo "Please start services with: docker-compose up -d"
    exit 1
fi

# Check database
check_database
db_status=$?

# Wait a moment for services to be ready
sleep 2

# Check backend service (GraphQL endpoint)
check_service "Backend GraphQL" "http://localhost:8080/graphql" 400
backend_status=$?

# Check edge server (should have some endpoint - let's try root)
check_service "Edge Server" "http://localhost:8081/" 404
edge_status=$?

# Summary
echo ""
echo "=== Health Check Summary ==="

total_checks=3
failed_checks=$((3 - db_status - backend_status - edge_status))

if [ $failed_checks -eq 0 ]; then
    echo -e "${GREEN}All services are healthy! ✓${NC}"
    check_docker_services
    exit 0
else
    echo -e "${RED}$failed_checks out of $total_checks services are unhealthy ✗${NC}"
    check_docker_services
    echo ""
    echo "Troubleshooting tips:"
    echo "- Check logs with: docker-compose logs"
    echo "- Restart services with: docker-compose restart"
    echo "- Check individual service logs:"
    echo "  - Backend: docker-compose logs feature_toggle_backend"
    echo "  - Edge: docker-compose logs feature_edge_server"
    echo "  - Database: docker-compose logs postgres_server"
    exit 1
fi
