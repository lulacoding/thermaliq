#!/bin/bash
# Sync project context files from brain/projects/thermaliq/
# Run this when brain context files are updated

set -e

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BRAIN_CONTEXT="$PROJECT_ROOT/../brain/projects/thermaliq"

if [ ! -d "$BRAIN_CONTEXT" ]; then
    echo "Error: Brain context not found at $BRAIN_CONTEXT"
    exit 1
fi

echo "Syncing context from $BRAIN_CONTEXT..."

mkdir -p "$PROJECT_ROOT/.claude/context"

cp "$BRAIN_CONTEXT/status.md" "$PROJECT_ROOT/.claude/context/status.md"
cp "$BRAIN_CONTEXT/decisions.md" "$PROJECT_ROOT/.claude/context/decisions.md"
cp "$BRAIN_CONTEXT/lessons.md" "$PROJECT_ROOT/.claude/context/lessons.md"

echo "✓ Context files synced from brain"
echo "  - status.md"
echo "  - decisions.md"
echo "  - lessons.md"
