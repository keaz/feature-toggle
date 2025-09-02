#!/bin/bash

# Test script to verify JWT authentication is working
# This script tests the login mutation and verifies JWT token is returned

echo "🚀 Testing JWT Authentication Implementation"
echo "============================================="

# Start the server in the background
echo "📦 Starting the feature-toggle backend server..."
cargo run &
SERVER_PID=$!

# Wait for server to start
echo "⏳ Waiting for server to start..."
sleep 5

# Test the login mutation
echo "🔐 Testing login mutation..."

GRAPHQL_QUERY='{
  "query": "mutation { login(input: { username: \"admin\", password: \"admin123\" }) { user { id username } token } }"
}'

# Make the request
RESPONSE=$(curl -s -X POST \
  -H "Content-Type: application/json" \
  -d "$GRAPHQL_QUERY" \
  http://localhost:8080/graphql)

echo "📨 Response:"
echo "$RESPONSE" | jq '.'

# Check if response contains a token
if echo "$RESPONSE" | jq -e '.data.login.token' > /dev/null; then
    echo "✅ JWT token successfully returned!"
    TOKEN=$(echo "$RESPONSE" | jq -r '.data.login.token')
    echo "🔑 Token: ${TOKEN:0:50}..."
else
    echo "❌ No JWT token found in response"
fi

# Test authenticated request with token
if [ ! -z "$TOKEN" ]; then
    echo "🔒 Testing authenticated request with JWT token..."
    
    AUTH_QUERY='{ "query": "query { teams { id name } }" }'
    
    AUTH_RESPONSE=$(curl -s -X POST \
      -H "Content-Type: application/json" \
      -H "Authorization: Bearer $TOKEN" \
      -d "$AUTH_QUERY" \
      http://localhost:8080/graphql)
    
    echo "📨 Authenticated response:"
    echo "$AUTH_RESPONSE" | jq '.'
    
    if echo "$AUTH_RESPONSE" | jq -e '.data.teams' > /dev/null; then
        echo "✅ Authenticated request successful!"
    else
        echo "❌ Authenticated request failed"
    fi
fi

# Test unauthenticated request
echo "🚫 Testing unauthenticated request..."
UNAUTH_RESPONSE=$(curl -s -X POST \
  -H "Content-Type: application/json" \
  -d "$AUTH_QUERY" \
  http://localhost:8080/graphql)

echo "📨 Unauthenticated response:"
echo "$UNAUTH_RESPONSE" | jq '.'

if echo "$UNAUTH_RESPONSE" | jq -e '.error' > /dev/null; then
    echo "✅ Unauthenticated request properly rejected!"
else
    echo "❌ Unauthenticated request was not properly rejected"
fi

# Cleanup
echo "🧹 Cleaning up..."
kill $SERVER_PID
wait $SERVER_PID 2>/dev/null

echo "✨ JWT authentication test completed!"
