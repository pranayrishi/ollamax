// @ts-check
"use strict";

// Ollama-Forge VSCode extension entry point.
//
// Pure JavaScript on purpose: it has no build step and no npm dependencies
// (the `vscode` module is provided by the host at runtime), so it runs
// straight from source in the Extension Development Host with zero
// `npm install`. See README.md for how to run.

const vscode = require("vscode");
const { ForgeBackend } = require("./backend");
const { ChatViewProvider } = require("./chatViewProvider");
const { ForgeAuth } = require("./auth");
const { HubViewProvider } = require("./hub");
const { ForgeTelemetry } = require("./telemetry");
const { VoiceNavigator } = require("./voice");

/** @param {vscode.ExtensionContext} context */
function activate(context) {
  const output = vscode.window.createOutputChannel("Ollama-Forge");
  const log = (m) => output.appendLine(m);
  log("Ollama-Forge extension activating");

  const backend = new ForgeBackend(log);
  const auth = new ForgeAuth(context, log);
  const telemetry = new ForgeTelemetry(auth, log);
  const provider = new ChatViewProvider(context, backend, log, auth, telemetry);
  const hub = new HubViewProvider(context, auth, log, telemetry, backend);
  const voice = new VoiceNavigator(context, backend, log);

  // One-time, honest telemetry disclosure (opt-out model). Shown once; the user
  // can turn it off immediately or in Settings.
  if (!context.globalState.get("forge.telemetryDisclosed")) {
    context.globalState.update("forge.telemetryDisclosed", true);
    vscode.window
      .showInformationMessage(
        "Ollama-Forge collects anonymous usage metadata (counts only — never your code or prompts) to power your web dashboard. You can turn it off anytime.",
        "Keep on",
        "Turn off"
      )
      .then((choice) => {
        if (choice === "Turn off") {
          vscode.workspace
            .getConfiguration("forge")
            .update("telemetry", false, vscode.ConfigurationTarget.Global);
        }
      });
  }

  context.subscriptions.push(
    output,
    vscode.window.registerWebviewViewProvider("forge.chatView", provider, {
      webviewOptions: { retainContextWhenHidden: true },
    }),
    vscode.window.registerWebviewViewProvider("forge.hubView", hub, {
      webviewOptions: { retainContextWhenHidden: true },
    }),
    vscode.commands.registerCommand("forge.focusChat", () =>
      vscode.commands.executeCommand("forge.chatView.focus")
    ),
    vscode.commands.registerCommand("forge.newChat", () => provider.newChat()),
    vscode.commands.registerCommand("forge.restartServer", () =>
      provider.restartBackend()
    ),
    vscode.commands.registerCommand("forge.attachSelection", () =>
      provider.attachSelection()
    ),
    vscode.commands.registerCommand("forge.attachFile", () =>
      provider.attachActiveFile()
    ),
    vscode.commands.registerCommand("forge.signIn", () => provider.signIn(false)),
    vscode.commands.registerCommand("forge.signInDevice", () => provider.signIn(true)),
    vscode.commands.registerCommand("forge.signOut", () => provider.signOut()),
    // Phase 2: voice-activated demo navigation.
    vscode.commands.registerCommand("forge.voiceNavigate", () => voice.open()),
    { dispose: () => voice.dispose() },
    // Make sure the backend process is killed and pending telemetry flushed.
    { dispose: () => backend.stop() },
    { dispose: () => telemetry.dispose() }
  );
}

function deactivate() {
  // Backend is torn down via the disposable registered in activate().
}

module.exports = { activate, deactivate };
