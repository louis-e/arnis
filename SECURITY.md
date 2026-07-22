# Security Policy

## Reporting Security Vulnerabilities

If you discover a security vulnerability in Arnis, please **DO NOT** create a public GitHub issue.

Instead, please email your findings to: [security@arnismc.com](mailto:security@arnismc.com)

Please include:
- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if any)

We will acknowledge your email within 48 hours and keep you updated on the fix progress.

## Security Best Practices

### For Users

1. **Keep Rust updated**: Run `rustup update` regularly
2. **Use official sources only**: Download from https://arnismc.com or GitHub only
3. **Validate downloads**: Check SHA-256 checksums for releases
4. **Report suspicious activity**: Contact security team immediately

### For Developers

1. **Dependency updates**: Run `cargo update` regularly and review changes
2. **Code review**: All changes require peer review before merging
3. **Clippy compliance**: Fix all warnings - run `cargo clippy -- -D warnings`
4. **Test coverage**: New code must include tests
5. **No hardcoded secrets**: Never commit credentials, API keys, or tokens

## Vulnerability Disclosure Timeline

- **Day 1**: Report received and acknowledged
- **Day 3-5**: Initial assessment completed
- **Day 7-14**: Fix development begins
- **Day 14-21**: Fix prepared and tested
- **Day 21-30**: Patch released publicly
- **Day 30+**: Public disclosure of details

## Dependencies

Arnis depends on several external crates. We monitor these for security issues:

- `tauri` - Desktop application framework
- `tokio` - Async runtime
- `serde` - Serialization
- `reqwest` - HTTP client
- And others listed in `Cargo.toml`

Security updates to dependencies are prioritized and applied quickly.

## Current Security Status

- ✅ Edition 2021 support for modern Rust security features
- ✅ Regular dependency audits
- ✅ GitHub Security alerts enabled
- ✅ Code scanning enabled

Run `cargo audit` locally to check for known vulnerabilities:

```bash
cargo install cargo-audit
cargo audit
```

## Questions?

For security-related questions, contact: [security@arnismc.com](mailto:security@arnismc.com)

---

**Last updated**: July 12, 2026