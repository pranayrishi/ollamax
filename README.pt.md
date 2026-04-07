# Ollama-Forge

**Camada de otimização para agentes locais de codificação que rodam em Ollama.**

> [English README](README.md) · [简体中文](README.zh.md) · [日本語](README.ja.md) · [Deutsch](README.de.md)

Se você quer assistência de IA para programar mas não quer enviar seu código
para terceiros, suas opções hoje são: configurar manualmente cada ferramenta
(Aider, Continue.dev, Cline, OpenHands, twinny ...), escolher manualmente o
modelo por tarefa, gerenciar VRAM manualmente e aceitar os defaults com que
cada ferramenta vem. **Ollama-Forge é a camada de otimização compartilhada
abaixo de todas elas.**

## Recursos principais

- **Local-first, totalmente gratuito** — toda a inferência passa pelo seu
  Ollama local. Fora o daemon do Ollama, o programa não faz nenhuma chamada
  de rede.
- **Defaults conscientes do hardware** — detecta RAM/VRAM na instalação e em
  runtime (NVIDIA / AMD / Apple Silicon / Intel / só CPU), escolhe um modelo
  apropriado e `num_ctx`, recusa carregar modelos que não cabem.
- **Disciplina de `keep_alive`** — mantém modelos quentes por longos períodos
  para que a segunda chamada não custe 15 segundos de cold-start.
- **Agente de pesquisa** — `forge research "<pergunta>"` roda um loop de
  uso de ferramentas via Ollama local + ferramentas públicas gratuitas
  (DuckDuckGo, Wikipedia, arXiv, HTTP cru). **Sem APIs pagas.**
- **Execução paralela heterogênea** — um único `forge build` pode rodar
  trabalho de arquitetura num modelo 32B *ao mesmo tempo* que código
  boilerplate roda num 3B. O roteador é VRAM-aware: se a soma não cabe no
  VRAM livre, ele recolhe para um modelo único.
- **Replay determinístico** — defina `FORGE_REPLAY_LOG=caminho` e cada
  chamada ao Ollama é gravada com o digest do modelo (do `/api/tags`),
  seed, temperature e o SHA-256 real do prompt e da resposta. Depois,
  `forge replay caminho` reproduz a sessão inteira byte por byte.
  **Esta é a vantagem de compliance** para indústrias reguladas (finanças,
  saúde, defesa, jurídico) — LLMs hospedados não conseguem fazer isso porque
  rotacionam pesos silenciosamente.
- **Always-rules persistentes** — coloque arquivos Markdown em
  `~/.config/ollama-forge/rules/` e eles são automaticamente prefixados a
  todo system prompt em todo comando.
- **Scanner de auditoria** — `forge audit <dir>` roda varredura regex de
  segredos sobre seus arquivos *antes* de mandá-los pro modelo, prevenindo
  vazamentos de credenciais para o contexto. Saída `--json` para CI.
- **Roda no hardware que você já tem** — Mac (Intel + Apple Silicon),
  Linux x86_64, qualquer lugar onde Ollama rode.

## Início rápido

Você precisa do [Ollama](https://ollama.com/download) instalado e com `ollama serve` rodando.

```bash
git clone https://github.com/pranayrishi/ollamax
cd ollamax
./install.sh                                  # compila com cargo, instala em ~/.local/forge/bin
forge status                                  # mostra hardware detectado + modelo recomendado
forge research "o que é consenso Raft"        # roda um loop completo do agente de pesquisa
```

A lista completa de comandos, referência de configuração, compatibilidade
SKILL.md, formato do log de replay e guia de contribuição estão no
**[README em inglês](README.md)**.

## Status

Pré-alfa (v0.1.0). Todos os comandos públicos da CLI têm cobertura de CI no
GitHub Actions em macOS e Linux, e os contratos centrais estão fixados por
100+ testes. De fato utilizável, mas ainda evoluindo rápido.

## Contribuição

PRs são bem-vindos. Comece por [CONTRIBUTING.md](CONTRIBUTING.md).
