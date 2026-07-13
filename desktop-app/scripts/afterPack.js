// electron-builder afterPack hook — replicates the Sattva AI unsigned-distribution
// method on macOS:
//   1. Turn the RunAsNode fuse OFF so a stray ELECTRON_RUN_AS_NODE (set by IDEs/
//      terminals) can't make the app exit immediately on launch.
//   2. AD-HOC sign every component (`codesign --sign -`) with hardened runtime +
//      entitlements, including the bundled `forge` engine. Ad-hoc signing does
//      NOT remove the first-launch Gatekeeper warning for a browser-downloaded
//      app (only paid Developer ID + notarization does), but it makes the app
//      internally consistent so it opens cleanly via right-click → Open instead
//      of failing with "app is damaged". This is exactly what Sattva does.
const { execSync } = require("child_process");
const fs = require("fs");
const path = require("path");
const { validateAfterPack } = require("./validate-package");

exports.default = async function afterPack(context) {
  // Must run on every release target. It verifies the post-copy Resources
  // layout, including the engine, local voice manifest, and spatial UI files.
  validateAfterPack(context);

  if (context.electronPlatformName !== "darwin") return;

  const appName = context.packager.appInfo.productFilename;
  const appPath = path.join(context.appOutDir, `${appName}.app`);
  const entitlements = path.join(__dirname, "..", "assets", "entitlements.mac.plist");
  console.log(`[afterPack] processing ${appPath}`);

  try {
    execSync(`npx --yes @electron/fuses write --app "${appPath}" RunAsNode=off`, { stdio: "inherit" });
    console.log("[afterPack] RunAsNode fuse disabled");
  } catch (e) {
    console.error("[afterPack] fuses step failed:", e.message);
    throw e;
  }

  const sign = (target) =>
    execSync(
      `codesign --force --sign - --options runtime --entitlements "${entitlements}" "${target}"`,
      { stdio: "inherit" }
    );

  try {
    const fw = path.join(appPath, "Contents/Frameworks");
    if (fs.existsSync(fw)) {
      for (const e of fs.readdirSync(fw)) {
        if (e.endsWith(".framework") || e.endsWith(".app") || e.endsWith(".dylib")) {
          sign(path.join(fw, e));
        }
      }
    }
    // Sign the bundled engine (needs the spawn/JIT entitlements to run).
    const engine = path.join(appPath, "Contents/Resources/bin/forge");
    if (fs.existsSync(engine)) {
      sign(engine);
      console.log("[afterPack] signed bundled forge engine");
    }
    // The release workflow stages whisper.cpp directly into Resources/voice.
    // Sign adjacent dylibs first (a static build normally has none), then the
    // CLI itself before the enclosing app. This keeps hardened-runtime macOS
    // packages valid whether a future upstream build is static or shared.
    const voiceDir = path.join(appPath, "Contents/Resources/voice");
    if (fs.existsSync(voiceDir)) {
      for (const entry of fs.readdirSync(voiceDir).sort()) {
        const target = path.join(voiceDir, entry);
        if (entry.toLowerCase().endsWith(".dylib") && fs.statSync(target).isFile()) {
          sign(target);
          console.log(`[afterPack] signed bundled Whisper dylib ${entry}`);
        }
      }
      const whisperCli = path.join(voiceDir, "whisper-cli");
      if (fs.existsSync(whisperCli)) {
        sign(whisperCli);
        console.log("[afterPack] signed bundled whisper-cli");
      }
    }
    sign(appPath); // the app last
    execSync(`codesign --verify --deep --strict "${appPath}"`, { stdio: "inherit" });
    console.log("[afterPack] ad-hoc signing complete");
  } catch (e) {
    console.error("[afterPack] signing failed:", e.message);
    throw e;
  }
};
