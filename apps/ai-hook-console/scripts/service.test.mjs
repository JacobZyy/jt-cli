import assert from "node:assert/strict"
import test from "node:test"

import { renderLaunchAgent, SERVICE_LABEL } from "./service.mjs"

test("renders a localhost-only owned LaunchAgent", () => {
  const plist = renderLaunchAgent({
    appPath: "/tmp/app & console",
    homePath: "/Users/demo",
    nextPath: "/tmp/next",
    nodePath: "/tmp/node",
  })

  assert.match(plist, new RegExp(SERVICE_LABEL))
  assert.match(plist, /jt-ai-hook-console launchd service/)
  assert.match(plist, /127\.0\.0\.1/)
  assert.match(plist, /\/tmp\/app &amp; console/)
  assert.doesNotMatch(plist, /0\.0\.0\.0/)
})
