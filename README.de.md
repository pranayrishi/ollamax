# Ollama-Forge

**Optimierungsschicht für lokale Coding-Agenten, die auf Ollama laufen.**

> [English README](README.md) · [简体中文](README.zh.md) · [日本語](README.ja.md) · [Português](README.pt.md)

Wenn du KI-Coding-Unterstützung möchtest, ohne deinen Code an Dritte zu schicken,
hast du heute folgende Optionen: jedes einzelne Tool (Aider, Continue.dev, Cline,
OpenHands, twinny ...) von Hand konfigurieren, pro Aufgabe manuell ein Modell
auswählen, VRAM manuell verwalten und die Defaults akzeptieren, mit denen jedes
Tool ausgeliefert wird. **Ollama-Forge ist die geteilte Optimierungsschicht
darunter.**

## Kernfunktionen

- **Local-First, komplett kostenlos** — alle Inferenz läuft über deinen lokalen
  Ollama. Außer dem Ollama-Daemon stellt das Programm keine Netzwerkverbindungen her.
- **Hardware-bewusste Defaults** — erkennt RAM/VRAM bei Installation und zur
  Laufzeit (NVIDIA / AMD / Apple Silicon / Intel / nur CPU), wählt ein passendes
  Modell und `num_ctx`, weigert sich Modelle zu laden, die nicht passen.
- **`keep_alive`-Disziplin** — hält Modelle dauerhaft warm, damit der zweite
  Aufruf nicht 15 Sekunden Cold-Start ist.
- **Research-Agent** — `forge research "<Frage>"` läuft eine Tool-Use-Schleife
  über lokales Ollama + freie öffentliche Tools (DuckDuckGo, Wikipedia, arXiv,
  reines HTTP). **Keine kostenpflichtigen APIs.**
- **Heterogene parallele Ausführung** — ein einziges `forge build` kann
  Architekturarbeit auf einem 32B und Boilerplate auf einem 3B *gleichzeitig*
  laufen lassen. Der VRAM-bewusste Router fällt auf ein einzelnes Modell zurück,
  wenn die Summe nicht in den freien VRAM passt.
- **Deterministisches Replay** — setze `FORGE_REPLAY_LOG=pfad` und jeder
  Ollama-Aufruf wird inklusive Modell-Digest (`/api/tags` digest), Seed,
  Temperature und echtem SHA-256 von Prompt und Response geloggt. Später kann
  `forge replay pfad` die ganze Session bit-genau reproduzieren. **Das ist der
  Compliance-Hebel für regulierte Branchen** (Finanzwesen, Gesundheit, Verteidigung,
  Recht) — gehostete LLMs können das nicht, weil sie ihre Gewichte still rotieren.
- **Persistente Always-Rules** — lege Markdown-Dateien in
  `~/.config/ollama-forge/rules/` ab; sie werden automatisch in jeden System-Prompt
  jedes Befehls eingefügt.
- **Audit-Scanner** — `forge audit <dir>` läuft Regex-Secret-Scans über deine
  Dateien, *bevor* sie an das Modell geschickt werden — um zu verhindern, dass
  Credentials in den Kontext leaken. `--json`-Output für CI.
- **Läuft auf Hardware, die du schon hast** — Mac (Intel + Apple Silicon),
  Linux x86_64, überall wo Ollama läuft.

## Schnellstart

Du brauchst [Ollama](https://ollama.com/download) installiert und mit `ollama serve` laufend.

```bash
git clone https://github.com/pranayrishi/ollamax
cd ollamax
./install.sh                                 # baut mit cargo, installiert nach ~/.local/forge/bin
forge status                                 # zeigt erkannte Hardware + empfohlenes Modell
forge research "Was ist Raft Consensus"      # führt eine vollständige Research-Agent-Schleife aus
```

Die vollständige Befehlsliste, Konfigurationsreferenz, SKILL.md-Kompatibilität,
Replay-Log-Format und Contributing-Guide findest du im
**[englischen README](README.md)**.

## Status

Pre-Alpha (v0.1.0). Alle öffentlichen CLI-Kommandos sind in CI auf macOS und
Linux GitHub Actions abgedeckt, die Kern-Contracts sind durch 100+ Tests
festgepinnt. Tatsächlich nutzbar, aber entwickelt sich schnell weiter.

## Mitarbeit

Pull Requests sind willkommen. Lies zuerst [CONTRIBUTING.md](CONTRIBUTING.md).
