# Ollama-Forge

**Ollama 上で動くローカルコーディングエージェントのための最適化レイヤー。**

> [English README](README.md) · [简体中文](README.zh.md) · [Deutsch](README.de.md) · [Português](README.pt.md)

AI コーディング支援は欲しいけど、コードベースを第三者に渡したくない —— 今日の選択肢は、
各ツール (Aider、Continue.dev、Cline、OpenHands、twinny ……) を手作業で設定し、
タスクごとにモデルを手動で選び、VRAM を手動で管理し、各ツールが出荷するデフォルト
設定を受け入れることです。**Ollama-Forge は、それらの下で共有される最適化レイヤーです。**

## 主な機能

- **ローカルファースト、完全無料** — すべての推論はローカルの Ollama 経由。
  Ollama デーモン以外、ネットワーク呼び出しは一切しません。
- **ハードウェア対応のデフォルト** — インストール時と実行時に RAM/VRAM
  (NVIDIA / AMD / Apple Silicon / Intel / CPU のみ) を検出し、適切なモデルと
  `num_ctx` を選択。VRAM に収まらないモデルは読み込みません。
- **`keep_alive` 管理** — モデルを長期間メモリ常駐させ、2 回目以降の呼び出しが
  15 秒のコールドスタートにならないようにします。
- **リサーチエージェント** — `forge research "<質問>"` がローカル Ollama と
  無料の公開ツール (DuckDuckGo、Wikipedia、arXiv、生の HTTP) を使ってツール呼び出し
  ループを実行します。**有料 API は一切使いません。**
- **異種並列実行** — 1 回の `forge build` でアーキテクチャ作業を 32B モデル上で、
  ボイラープレートコードを 3B モデル上で *同時に* 実行できます。VRAM を見て、
  合計サイズが空き VRAM を超える場合は自動的に単一モデルにフォールバックします。
- **決定論的リプレイ** — `FORGE_REPLAY_LOG=path` を設定すると、すべての Ollama
  呼び出しがモデルダイジェスト (`/api/tags` digest) + シード + temperature + 実際の
  SHA-256 と共にログされます。あとで `forge replay path` がセッション全体を
  バイト単位で再現できます。**これは規制業界 (金融、医療、防衛、法律) 向けの
  コンプライアンスの切り札です** —— ホストされた LLM は重みをサイレントに
  ローテーションするためこれをできません。
- **永続的な always-rules** — Markdown ファイルを `~/.config/ollama-forge/rules/`
  にドロップすると、すべてのコマンドのシステムプロンプトに自動的に挿入されます。
- **監査スキャナ** — `forge audit <dir>` がファイルをモデルに送る前に正規表現で
  シークレットスキャンを実行し、認証情報がコンテキストに漏れるのを防ぎます。
  CI 用の `--json` 出力もあります。
- **すでに持っているハードウェアで動作** — Mac (Intel + Apple Silicon)、Linux x86_64、
  Ollama が動くすべての環境で動きます。

## クイックスタート

[Ollama](https://ollama.com/download) をインストールし、`ollama serve` が動いている必要があります。

```bash
git clone https://github.com/pranayrishi/ollamax
cd ollamax
./install.sh                              # cargo でビルド、~/.local/forge/bin にインストール
forge status                              # 検出されたハードウェア + 推奨モデルを表示
forge research "Raft コンセンサスとは"     # フルのリサーチエージェントループを実行
```

完全なコマンドリスト、設定リファレンス、SKILL.md 互換性、リプレイログ形式、
コントリビューションガイドは **[英語版 README](README.md)** を参照してください。

## ステータス

プレアルファ (v0.1.0)。CLI の公開コマンドは macOS と Linux の GitHub Actions
で CI カバーされており、コア契約は 100+ のテストでピン止めされています。
実用可能ですが、まだ高速で進化しています。

## コントリビューション

PR は歓迎です。まず [CONTRIBUTING.md](CONTRIBUTING.md) をお読みください。
