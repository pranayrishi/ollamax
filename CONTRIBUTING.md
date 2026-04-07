# Contributing to Ollama-Forge

Thank you for your interest in contributing to Ollama-Forge!

## Development Setup

1. Install Rust 1.75+

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

2. Install Ollama

```bash
curl -fsSL https://ollama.ai/install.sh | sh
```

3. Clone and build

```bash
git clone https://github.com/ollama-forge/ollama-forge
cd ollama-forge
cargo build --release
```

4. Pull models

```bash
ollama pull llama3.2:3b
ollama pull qwen2.5-coder:7b
ollama pull deepseek-coder-v2:16b
```

## Running Tests

```bash
cargo test
cargo test --release
cargo clippy
```

## Project Structure

```
src/
├── cli/          # Command-line interface
├── orchestrator/ # Task orchestration engine
├── router/       # Intelligent task routing
├── executor/     # Parallel execution engine
├── context/      # Context management
├── providers/    # LLM provider integrations
├── monitoring/   # VRAM and hardware monitoring
├── security/     # Security guardrails
└── skills/       # Skills marketplace
```

## Code Style

- Run `cargo fmt` before committing
- Follow Rust idioms and best practices
- Add tests for new functionality
- Document public APIs

## Submitting Changes

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Run tests
5. Submit a pull request

## Skills Contributions

Submit new skills to `skills/recipes/`:

```json
{
    "name": "your-skill",
    "description": "What it does",
    "tags": ["relevant", "tags"],
    "prompts": {...},
    "settings": {...},
    "recipes": [...]
}
```

## Questions?

Open an issue or join our discussions.
