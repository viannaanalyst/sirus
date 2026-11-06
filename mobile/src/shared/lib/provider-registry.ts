import type { AgentProviderId, ApprovalMode } from "../client/types";

export interface ProviderDefinition {
  id: AgentProviderId;
  name: string;
  vendor: string;
  website: string;
  capabilities: string[];
  executionNote?: string;
  approvalPolicy: "host" | "auto-review" | "vendor";
  approvalModes: ApprovalMode[];
}

export const PROVIDERS: ProviderDefinition[] = [
  {
    id: "claude",
    approvalModes: ["ask", "auto", "full"],
    approvalPolicy: "host",
    name: "Claude Code",
    vendor: "Anthropic",
    website: "https://docs.anthropic.com/en/docs/claude-code",
    capabilities: ["cli", "workspace-edits"],
  },
  {
    id: "codex",
    approvalModes: ["ask", "auto", "full"],
    approvalPolicy: "host",
    name: "Codex",
    vendor: "OpenAI",
    website: "https://github.com/openai/codex",
    capabilities: ["cli", "workspace-edits"],
  },
  {
    id: "cursor",
    approvalModes: ["ask", "auto", "full"],
    approvalPolicy: "host",
    name: "Cursor",
    vendor: "Cursor",
    website: "https://cursor.com/docs/cli/acp",
    capabilities: ["cli", "workspace-edits", "native-resume"],
    executionNote: "Cursor runs through ACP with native sessions that resume exactly. Request approval asks here before its tools; Auto-review allows file edits and asks before commands; Full access allows every request. Planning uses Cursor's Plan mode. Vendor deny rules remain effective.",
  },
  {
    id: "grok",
    approvalModes: [],
    approvalPolicy: "vendor",
    name: "Grok",
    vendor: "xAI",
    website: "https://docs.x.ai/build/cli/overview",
    capabilities: ["cli", "workspace-edits"],
  },
  {
    id: "opencode",
    approvalModes: ["ask", "auto", "full"],
    approvalPolicy: "host",
    name: "OpenCode",
    vendor: "OpenCode",
    website: "https://opencode.ai/docs/acp/",
    capabilities: ["cli", "workspace-edits", "native-resume", "host-file-approvals"],
    executionNote: "OpenCode uses native sessions. Request approval reviews file edits; Auto-review allows file edits within the adapter policy. Full access allows CLI tools. Unsupported host callbacks are denied. ACP is not a filesystem sandbox.",
  },
  {
    id: "antigravity", name: "Antigravity", vendor: "Google", website: "https://antigravity.google/docs/cli/install/",
    capabilities: ["cli", "workspace-edits"], approvalPolicy: "vendor", approvalModes: [],
    executionNote: "Antigravity uses its CLI permissions and terminal sandbox. Headless permission requests are denied by the CLI; this adapter cannot answer them. Its terminal sandbox is not a filesystem sandbox for every tool.",
  },
  {
    id: "droid", name: "Droid", vendor: "Factory", website: "https://docs.factory.ai/droid-cli/overview",
    capabilities: ["cli", "workspace-edits"], approvalPolicy: "vendor", approvalModes: ["auto"],
    executionNote: "Droid Auto-review uses low autonomy for file edits. Planning uses the CLI's read-only default. Higher autonomy and permission bypasses are unavailable in this adapter.",
  },
  {
    id: "pi", name: "Pi", vendor: "Earendil", website: "https://pi.dev/docs/latest/quickstart",
    capabilities: ["cli", "workspace-edits"], approvalPolicy: "vendor", approvalModes: ["auto"],
    executionNote: "Pi uses built-in read/edit/write tools; planning selects read-only tools. Shell tools and extensions are disabled. Tool selection is not an operating-system filesystem sandbox.",
  },
  {
    id: "devin", name: "Devin", vendor: "Cognition", website: "https://docs.devin.ai/cli/index",
    capabilities: ["cli", "workspace-edits", "native-resume"], approvalPolicy: "host", approvalModes: ["ask", "auto", "full"],
    executionNote: "Devin runs through ACP with native sessions that resume exactly, using the CLI's own sign-in. Request approval and Auto-review run in Devin's Code mode: it accepts file edits and asks here before commands, each answer once only. Full access uses Bypass permissions; planning uses Plan mode, which denies writes. It does not start cloud sessions.",
  },
  {
    id: "hermes", name: "Hermes", vendor: "Nous Research", website: "https://hermes-agent.nousresearch.com/docs/",
    capabilities: ["cli", "workspace-edits", "native-resume"], approvalPolicy: "host", approvalModes: ["ask", "auto", "full"],
    executionNote: "Hermes Agent runs through ACP with native sessions that resume exactly, using the model provider set with `hermes model`. Request approval asks here for every tool; Auto-review accepts file edits; Full access runs without asking. Planning asks for everything and denies writes.",
  },
];

export function providerById(id: AgentProviderId) {
  return PROVIDERS.find((item) => item.id === id) ?? PROVIDERS[0];
}
