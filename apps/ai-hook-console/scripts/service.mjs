import { spawnSync } from "node:child_process"
import { createRequire } from "node:module"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import {
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs"
import { getuid } from "node:process"
import { homedir } from "node:os"

export const SERVICE_LABEL = "com.jacob.jt-ai-hook-console"
const OWNERSHIP_MARKER = "jt-ai-hook-console launchd service"
const appDirectory = fileURLToPath(new URL("..", import.meta.url))
const require = createRequire(import.meta.url)
const nextBin = require.resolve("next/dist/bin/next")
const home = homedir()
const launchAgentsDirectory = join(home, "Library", "LaunchAgents")
const logsDirectory = join(home, "Library", "Logs")
const plistPath = join(launchAgentsDirectory, `${SERVICE_LABEL}.plist`)
const serviceTarget = `gui/${getuid()}/${SERVICE_LABEL}`

function escapeXml(value) {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&apos;")
}

export function renderLaunchAgent({ appPath, homePath, nextPath, nodePath }) {
  const logPath = join(homePath, "Library", "Logs", "jt-ai-hook-console.log")
  const errorLogPath = join(homePath, "Library", "Logs", "jt-ai-hook-console.error.log")
  const environmentPath = [dirname(nodePath), "/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"].join(":")
  const value = escapeXml
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- ${OWNERSHIP_MARKER} -->
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>${SERVICE_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>${value(nodePath)}</string>
    <string>${value(nextPath)}</string>
    <string>start</string>
    <string>--hostname</string>
    <string>127.0.0.1</string>
    <string>--port</string>
    <string>3100</string>
  </array>
  <key>WorkingDirectory</key>
  <string>${value(appPath)}</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>NODE_ENV</key>
    <string>production</string>
    <key>PATH</key>
    <string>${value(environmentPath)}</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>ProcessType</key>
  <string>Background</string>
  <key>ThrottleInterval</key>
  <integer>5</integer>
  <key>StandardOutPath</key>
  <string>${value(logPath)}</string>
  <key>StandardErrorPath</key>
  <string>${value(errorLogPath)}</string>
</dict>
</plist>
`
}

function runLaunchctl(arguments_, allowFailure = false) {
  const result = spawnSync("launchctl", arguments_, { encoding: "utf8" })
  if (result.status !== 0 && !allowFailure) {
    throw new Error((result.stderr || result.stdout || `launchctl ${arguments_.join(" ")} failed`).trim())
  }
  return result
}

function isLoaded() {
  return runLaunchctl(["print", serviceTarget], true).status === 0
}

function assertOwnedPlist() {
  if (!existsSync(plistPath)) return
  const content = readFileSync(plistPath, "utf8")
  if (!content.includes(OWNERSHIP_MARKER) && !(content.includes(SERVICE_LABEL) && content.includes(appDirectory))) {
    throw new Error(`Refusing to replace unowned LaunchAgent: ${plistPath}`)
  }
}

function writePlist() {
  mkdirSync(launchAgentsDirectory, { recursive: true })
  mkdirSync(logsDirectory, { recursive: true })
  const temporaryPath = `${plistPath}.tmp-${process.pid}`
  writeFileSync(temporaryPath, renderLaunchAgent({
    appPath: appDirectory,
    homePath: home,
    nextPath: nextBin,
    nodePath: process.execPath,
  }), { mode: 0o644 })
  renameSync(temporaryPath, plistPath)
}

function stopLoadedService() {
  if (!isLoaded()) return
  runLaunchctl(["bootout", serviceTarget])
  const result = spawnSync("/bin/sleep", ["0.5"])
  if (result.status !== 0) throw new Error("Could not wait for LaunchAgent shutdown.")
}

function install() {
  if (process.platform !== "darwin") throw new Error("AI-hook console service supports macOS only.")
  if (!existsSync(join(appDirectory, ".next", "BUILD_ID"))) {
    throw new Error("Production build missing. Run `pnpm run build` first.")
  }
  assertOwnedPlist()
  stopLoadedService()
  writePlist()
  runLaunchctl(["enable", serviceTarget])
  runLaunchctl(["bootstrap", `gui/${getuid()}`, plistPath])
  runLaunchctl(["kickstart", "-k", serviceTarget])
  console.log("AI-hook console running at http://127.0.0.1:3100")
}

function restart() {
  if (!existsSync(plistPath) || !isLoaded()) {
    install()
    return
  }
  runLaunchctl(["kickstart", "-k", serviceTarget])
  console.log("AI-hook console restarted at http://127.0.0.1:3100")
}

function uninstall() {
  assertOwnedPlist()
  stopLoadedService()
  rmSync(plistPath, { force: true })
  console.log("AI-hook console service removed.")
}

function status() {
  if (!isLoaded()) {
    console.log("AI-hook console service stopped.")
    return
  }
  const result = runLaunchctl(["print", serviceTarget])
  process.stdout.write(result.stdout)
}

function main() {
  const command = process.argv[2]
  const actions = { install, restart, status, uninstall }
  if (!(command in actions)) {
    throw new Error("Usage: service.mjs <install|restart|status|uninstall>")
  }
  actions[command]()
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  try {
    main()
  }
  catch (error) {
    console.error(error instanceof Error ? error.message : String(error))
    process.exitCode = 1
  }
}
