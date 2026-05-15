# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Initial project structure and architecture
- Unified URI context management system
- Three-layer summary mechanism (L0/L1/L2)
- Virtual file system for context storage
- Qdrant integration for vector storage
- Multi-model support (OpenAI, Claude, DeepSeek)
- Model routing with failover support
- CLI interface with multiple commands
- Knowledge ingestion system
- Memory management with decay mechanism
- Skill system framework
- Configuration management
- Logging system
- Security features

### Changed
- N/A

### Deprecated
- N/A

### Removed
- N/A

### Fixed
- N/A

### Security
- N/A

## [0.1.0] - 2024-01-15

### Added
- Initial release
- Basic CLI interface with `chat`, `search`, `ingest` commands
- OpenAI model support
- Local file storage backend
- Basic memory system
- Configuration file support
- Environment variable configuration

### Architecture
- Unified context storage with `tianyan://` URI scheme
- Three-layer summary for efficient context retrieval
- Virtual file system mapping to local storage
- Qdrant-based vector storage for semantic search

### Documentation
- README with installation and usage instructions
- Configuration guide
- Development guide
- Example configuration files

### Infrastructure
- GitHub Actions CI/CD pipeline
- Cross-platform build support (Linux, macOS, Windows)
- Installation scripts for all platforms

---

## Version History

| Version | Date | Description |
|---------|------|-------------|
| 0.1.0 | 2024-01-15 | Initial release |

---

## Upgrade Guide

### From 0.1.0 to Unreleased

No breaking changes in the unreleased version.

---

## Roadmap

### v0.2.0 (Planned)
- GUI interface (egui-based)
- Web interface
- Plugin system for skills
- Multi-agent collaboration

### v0.3.0 (Planned)
- Knowledge graph support
- Advanced memory consolidation
- Custom embedding models
- Distributed storage support

### v1.0.0 (Future)
- Stable API
- Production-ready
- Comprehensive documentation
- Full test coverage

---

## Contributing

See [Development Guide](./docs/development.md) for information on how to contribute.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
