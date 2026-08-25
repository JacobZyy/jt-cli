import type { NextConfig } from "next"

const nextConfig: NextConfig = {
  agentRules: false,
  transpilePackages: ["@workspace/ai-hook-core", "@workspace/ui"],
}

export default nextConfig
