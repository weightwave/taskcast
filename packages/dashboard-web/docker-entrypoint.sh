#!/bin/sh
set -e

# Generate /api/config JSON with properly escaped env var using jq
jq -n --arg url "${TASKCAST_SERVER_URL}" '{ baseUrl: $url }' \
  > /usr/share/nginx/html/api-config.json

exec nginx -g 'daemon off;'
