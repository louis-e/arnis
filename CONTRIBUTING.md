# Contributing to Arnis

Thank you for your interest in contributing to Arnis! This document provides guidelines and instructions for contributing to the project.

## 📋 Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Getting Started](#getting-started)
- [Development Setup](#development-setup)
- [Making Changes](#making-changes)
- [Submitting Changes](#submitting-changes)
- [Coding Standards](#coding-standards)
- [Testing](#testing)

## Code of Conduct

We are committed to providing a welcoming and inclusive environment for all contributors. Please be respectful and constructive in all interactions.

## Getting Started

### Prerequisites

- Rust 1.56+ (for Edition 2021 support)
- Cargo
- Git
- Docker (optional but recommended)

### Fork and Clone

```bash
# Fork the repository on GitHub
# Clone your fork
git clone https://github.com/YOUR_USERNAME/arnis.git
cd arnis

# Add upstream remote
git remote add upstream https://github.com/louis-e/arnis.git
```

## Development Setup

### Option 1: Docker (Recommended)

```bash
# Build the Docker image
docker build -t arnis:latest .

# Run in development mode
docker run -it --rm -v $(pwd):/build arnis:latest bash
cd /build
cargo build
```

### Option 2: Dev Container (VS Code)

```bash
# Open in VS Code
code .

# Press Ctrl+Shift+P and select "Dev Containers: Reopen in Container"
# Wait for container to build and start
```

### Option 3: Local Installation

```bash
# Update Rust
rustup update

# Install system dependencies (Linux/Debian)
sudo apt-get install -y \
  build-essential \
  libssl-dev \
  pkg-config \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev

# Or on macOS
brew install webkit2gtk

# Build the project
cargo build
```

## Making Changes

### Create a Feature Branch

```bash
# Update main branch
git fetch upstream
git checkout main
git merge upstream/main

# Create feature branch
git checkout -b feature/your-feature-name
```

### Development Workflow

```bash
# Build debug version
cargo build

# Run tests
cargo test

# Format code
cargo fmt

# Lint with Clippy
cargo clippy

# Run the GUI
cargo run

# Run CLI with custom options
cargo run --no-default-features -- --terrain --path="path/to/world" --bbox="min_lat,min_lng,max_lat,max_lng"
```

### Code Organization

The project is organized into modules:

```
src/
├── main.rs              # Entry point
├── cli/                 # Command-line interface
├── gui/                 # Tauri GUI
├── generation/          # World generation logic
├── data/                # Data fetching and processing
├── minecraft/           # Minecraft-specific code
└── utils/              # Utility functions
```

## Submitting Changes

### Before You Submit

1. **Ensure tests pass:**
   ```bash
   cargo test
   ```

2. **Format your code:**
   ```bash
   cargo fmt
   ```

3. **Run Clippy:**
   ```bash
   cargo clippy -- -D warnings
   ```

4. **Update documentation** if needed

5. **Create meaningful commits:**
   ```bash
   git commit -m "feat: add feature description"
   git commit -m "fix: resolve issue #123"
   git commit -m "docs: update README"
   ```

### Push and Create Pull Request

```bash
# Push to your fork
git push origin feature/your-feature-name

# Create PR via GitHub web interface
# Reference any related issues: "Fixes #123"
```

### PR Guidelines

- Keep PRs focused on a single feature or fix
- Write clear PR descriptions
- Link related issues
- Ensure CI/CD checks pass
- Be responsive to review feedback

## Coding Standards

### Rust Style

- Follow standard Rust conventions
- Use `cargo fmt` for formatting
- Use `cargo clippy` for linting
- Write clear, self-documenting code

### Documentation

- Add doc comments to public functions:
  ```rust
  /// Generates a Minecraft world from geographic data.
  ///
  /// # Arguments
  ///
  /// * `bbox` - Bounding box coordinates
  /// * `scale` - World scale factor
  ///
  /// # Returns
  ///
  /// Returns a `Result` with the generated world path
  pub fn generate_world(bbox: BBox, scale: f64) -> Result<String> {
      // implementation
  }
  ```

- Update in-code comments for complex logic
- Keep README and Wiki updated

### Git Commit Messages

Follow conventional commits:

```
type(scope): subject

body

footer
```

Types:
- `feat:` - New feature
- `fix:` - Bug fix
- `docs:` - Documentation
- `style:` - Code style (formatting)
- `refactor:` - Code refactoring
- `perf:` - Performance improvement
- `test:` - Adding or updating tests
- `ci:` - CI/CD changes

Example:
```
feat(generation): add support for custom terrain scaling

- Implement terrain scale factor configuration
- Add validation for scale boundaries
- Update GUI with new control

Fixes #456
```

## Testing

### Running Tests

```bash
# Run all tests
cargo test

# Run specific test
cargo test test_name

# Run with output
cargo test -- --nocapture

# Run ignored tests
cargo test -- --ignored
```

### Writing Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_world_generation() {
        let bbox = BBox::new(0.0, 0.0, 1.0, 1.0);
        let result = generate_world(bbox, 1.0);
        assert!(result.is_ok());
    }

    #[test]
    #[should_panic]
    fn test_invalid_bbox() {
        let bbox = BBox::new(1.0, 1.0, 0.0, 0.0); // invalid
        generate_world(bbox, 1.0).unwrap();
    }
}
```

## Areas for Contribution

### High Priority

- [ ] Performance optimization for large worlds
- [ ] Additional data source support
- [ ] iOS companion app (mobile access)
- [ ] Improved error handling and user feedback
- [ ] Enhanced documentation

### Medium Priority

- [ ] Additional Minecraft version support
- [ ] New terrain generation algorithms
- [ ] Caching improvements
- [ ] UI/UX enhancements

### Community Contributions Welcome

- Bug reports with reproduction steps
- Feature requests with use cases
- Documentation improvements
- Translation support
- Platform-specific testing

## Need Help?

- 📖 Check the [GitHub Wiki](https://github.com/louis-e/arnis/wiki/)
- 🐛 Search existing [Issues](https://github.com/louis-e/arnis/issues)
- 💬 Start a [Discussion](https://github.com/louis-e/arnis/discussions)
- 📧 Contact maintainers

## License

By contributing to Arnis, you agree that your contributions will be licensed under the Apache License 2.0 (same as the project).

---

**Happy contributing! 🚀**
