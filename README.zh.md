# Ollama-Forge

**为运行在 Ollama 上的本地代码代理打造的优化层。**

> [English README](README.md) · [日本語](README.ja.md) · [Deutsch](README.de.md) · [Português](README.pt.md)

如果你想要 AI 辅助编码,但又不想把代码库交给第三方,你今天的选择是:
手工配置每个工具(Aider、Continue.dev、Cline、OpenHands、twinny……)、
按任务手动选择模型、手动管理 VRAM、并接受每个工具默认的设置。
**Ollama-Forge 是它们底下共享的优化层。**

## 核心能力

- **本地优先,完全免费** — 所有调用都走本地的 Ollama。除了 Ollama 守护进程,
  程序不发出任何网络请求(可通过环境变量 `FORGE_IGNORE_ROBOTS=1` 等显式控制)。
- **硬件感知的默认值** — 在安装和运行时检测你的 RAM/VRAM(NVIDIA / AMD /
  Apple Silicon / Intel / 纯 CPU),自动选择合适的模型和 `num_ctx`,
  拒绝加载装不下的模型。
- **`keep_alive` 调度** — 长期保持模型常驻,第二次调用不再花 15 秒冷启动。
- **研究代理** — `forge research "<问题>"` 用本地 Ollama + 免费的公开工具
  (DuckDuckGo、Wikipedia、arXiv、原生 HTTP)运行一个工具调用循环。**没有付费 API**。
- **异构并行执行** — 同一个 `forge build` 可以让架构任务跑在 32B 上,
  样板代码并行跑在 3B 上。VRAM 感知的路由器会在显存不够时自动回退到单模型。
- **确定性回放** — 把 `FORGE_REPLAY_LOG=path` 设上,每次 Ollama 调用都会
  连同模型摘要 (`/api/tags` digest) + seed + temperature + 真实 SHA-256 一起记录。
  之后 `forge replay path` 可以位级别地复现整个会话。**这是面向监管行业
  (金融、医疗、国防、法律)的合规筹码** —— 托管 LLM 因为权重静默轮换无法做到这点。
- **持久化的 always-rules** — 把 Markdown 文件丢进 `~/.config/ollama-forge/rules/`,
  它们会被自动注入到每条命令的系统提示词中。
- **审计扫描器** — `forge audit <dir>` 在文件被发送给模型之前先做正则秘钥扫描,
  防止意外把凭据泄漏到上下文。`--json` 输出可以接 CI。
- **运行在你已经有的硬件上** — Mac (Intel + Apple Silicon)、Linux x86_64,
  以及任何能跑 Ollama 的地方。

## 快速开始

需要先装好 [Ollama](https://ollama.com/download) 并运行 `ollama serve`。

```bash
git clone https://github.com/pranayrishi/ollamax
cd ollamax
./install.sh                        # 用 cargo 构建,装到 ~/.local/forge/bin
forge status                        # 查看检测到的硬件 + 推荐模型
forge research "什么是 Raft 共识"    # 跑一个完整的研究代理循环
```

完整的命令列表、配置参考、SKILL.md 兼容性说明、回放日志格式以及贡献指南请见
**[英文 README](README.md)**。

## 状态

预 Alpha (v0.1.0)。命令行的所有公开命令都在 macOS 和 Linux 上的 GitHub Actions
上有 CI 覆盖,核心契约由 100+ 个测试钉住。代码库实际可用,但还在快速迭代。

## 贡献

欢迎 PR。请先看 [CONTRIBUTING.md](CONTRIBUTING.md)。
