# Documentation Maintenance Guide

This guide ensures documentation stays synchronized with code changes.

## 🔄 Documentation Update Process

### When to Update Documentation

**Mandatory Updates (MUST DO):**
- ✅ New features implemented
- ✅ Configuration changes
- ✅ API modifications
- ✅ Breaking changes
- ✅ Security fixes
- ✅ Performance improvements

**Recommended Updates (SHOULD DO):**
- 🔄 Bug fixes that affect user behavior
- 🔄 Code refactoring that changes interfaces
- 🔄 Error handling improvements
- 🔄 New example configurations

### Update Checklist

For every code change, run through this checklist:

#### [ ] Feature Implementation
```bash
# 1. Update requirements log
echo "RXXX: New feature description" >> ../requirements/README.md

# 2. Update relevant documentation
edit configuration.md    # If config changes
edit examples.md        # Add new examples
edit quick-start.md     # Update quick start
edit api.md             # If API changes
```

#### [ ] Configuration Changes
```bash
# 1. Update configuration guide
edit docs/configuration.md

# 2. Add/update examples
edit examples/config_new_feature.json

# 3. Update changelog
edit CHANGELOG.md
```

#### [ ] Bug Fixes
```bash
# 1. Update changelog
edit CHANGELOG.md

# 2. Update troubleshooting if applicable
edit docs/installation.md

# 3. Add note to requirements
echo "RXXX: Bug fix description" >> ../requirements/README.md
```

## 📝 Documentation Templates

### Feature Addition Template
```markdown
## [Version] - YYYY-MM-DD

### Added
- **Feature Name** (RXXX)
  - Brief description of the feature
  - How to use it with examples
  - Configuration options
  - CLI arguments if applicable

- Updated documentation:
  - [x] Configuration guide
  - [x] Examples
  - [x] Quick start
```

### Configuration Change Template
```json
{
  "change_description": "Added new configuration option",
  "field_name": "new_field",
  "type": "boolean | string | number | array | object",
  "required": true | false,
  "default": "default_value",
  "example": "example_usage"
}
```

### Example Configuration Template
```json
{
  "description": "Feature name example configuration",
  "use_case": "When to use this configuration",
  "config": {
    "mode": "Reverse",
    "listen_addr": "127.0.0.1:8080",
    "static_files": {
      "mounts": [
        {
          "path": "/feature",
          "root_dir": "./feature-dist",
          "new_field": true
        }
      ]
    }
  }
}
```

## 🚀 Pre-Commit Checklist

Create a pre-commit hook (`.git/hooks/pre-commit`):

```bash
#!/bin/bash
# Pre-commit documentation check

echo "🔍 Checking documentation updates..."

# Check if documentation needs updating
CHANGED_FILES=$(git diff --cached --name-only)

if echo "$CHANGED_FILES" | grep -q "src/"; then
    echo "📝 Source code changes detected. Please ensure documentation is updated:"
    echo "  - [ ] docs/configuration.md if config changed"
    echo "  - [ ] docs/examples.md if new examples needed"
    echo "  - [ ] CHANGELOG.md for version tracking"
    echo "  - [ ] requirements/README.md for requirements log"

    read -p "Continue with commit? (y/N): " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        exit 1
    fi
fi

echo "✅ Documentation check passed"
```

## 📋 Monthly Documentation Review

### Review Checklist
- [ ] All new features documented
- [ ] Examples are current and working
- [ ] Configuration guide is up-to-date
- [ ] Installation instructions are tested
- [ ] Troubleshooting section covers common issues
- [ ] API documentation matches implementation
- [ ] Performance information is current
- [ ] Security best practices are documented

### Review Process
```bash
# Monthly review script
#!/bin/bash

echo "📚 Starting monthly documentation review..."

# 1. Check examples work
cargo run --config examples/config_spa.json &
SERVER_PID=$!
sleep 2
curl -s http://127.0.0.1:8080/ > /dev/null
if [ $? -eq 0 ]; then
    echo "✅ SPA example works"
else
    echo "❌ SPA example failed"
fi
kill $SERVER_PID 2>/dev/null

# 2. Check configuration syntax
for config in examples/*.json; do
    if cat "$config" | python -m json.tool > /dev/null 2>&1; then
        echo "✅ $config has valid JSON"
    else
        echo "❌ $config has invalid JSON"
    fi
done

# 3. Check links in documentation
# (Requires markdown-link-check)
markdown-link-check docs/*.md

echo "📊 Documentation review complete"
```

## 🔄 Version Release Documentation

### Pre-Release Checklist
- [ ] CHANGELOG.md updated with all changes
- [ ] Version number bumped in Cargo.toml
- [ ] All new examples tested
- [ ] Migration guide prepared for breaking changes
- [ ] API documentation current
- [ ] Performance benchmarks updated

### Release Documentation Tasks
```bash
# 1. Update version references
find docs/ -type f -name "*.md" -exec sed -i "s/version [0-9]\+\.[0-9]\+\.[0-9]\+/version $NEW_VERSION/g" {} \;

# 2. Generate API docs
cargo doc --no-deps --open

# 3. Test all examples
for config in examples/*.json; do
    echo "Testing $config..."
    cargo run --config "$config" &
    sleep 2
    # Basic health check
    curl -s http://127.0.0.1:8080/ > /dev/null
    if [ $? -eq 0 ]; then
        echo "✅ $config works"
    else
        echo "❌ $config failed"
    fi
    pkill -f "proxy-server"
done

# 4. Archive old versions
mkdir -p docs/archive
mv CHANGELOG.md "docs/archive/CHANGELOG-v$OLD_VERSION.md"
cp CHANGELOG.md CHANGELOG.md.new
```

## 📊 Documentation Metrics

### Track These Metrics
- Number of documented features vs implemented features
- Example configuration success rate
- Documentation coverage (lines of docs vs lines of code)
- User feedback on documentation clarity
- Time from feature implementation to documentation update

### Improvement Goals
- 100% feature documentation coverage
- All examples tested and working
- Documentation updated within 24 hours of code changes
- User-reported documentation issues < 5% of total issues

## 🤝 Contributing to Documentation

### Documentation Types
1. **User Documentation** - Installation, configuration, usage
2. **Developer Documentation** - Architecture, contributing, testing
3. **API Documentation** - Code-level documentation
4. **Examples** - Working configuration examples

### Writing Guidelines
- Use clear, simple language
- Include working examples
- Cross-reference related topics
- Update dates and versions
- Test all code examples

### Review Process
1. Technical review for accuracy
2. User review for clarity
3. Example testing for functionality
4. Final approval before merge

---

**Remember:** Documentation is as important as code. Outdated documentation is worse than no documentation.

**Last Updated:** 2025-11-15
**Next Review:** 2025-12-15