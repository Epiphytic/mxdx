#!/bin/bash
set -euo pipefail

# Publish npm packages in dependency order

echo "Publishing @mxdx/core..."
cd packages/core && npm publish --provenance --access public && cd ../..

echo "Publishing @mxdx/launcher..."
cd packages/launcher && npm publish --provenance --access public && cd ../..

echo "Publishing @mxdx/client..."
cd packages/client && npm publish --provenance --access public && cd ../..

echo "Publishing @mxdx/web-console..."
cd packages/web-console && npm publish --provenance --access public && cd ../..

echo "Publishing mxdx..."
cd packages/mxdx && npm publish --provenance --access public && cd ../..

echo "All npm packages published."
