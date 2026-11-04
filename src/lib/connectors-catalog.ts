import type { ConnectorAuth } from "@/client/types";
export { CONNECTOR_AGENTS, normalizeConnectors, validConnectorUrl } from "./connectors";
import canvaIcon from "@/assets/connectors/canva.png";
import context7Icon from "@/assets/connectors/context7.png";
import mondayIcon from "@/assets/connectors/monday.png";

/**
 * The connector catalog (ADR-137): each service's official remote MCP server. All of them sign
 * in with the MCP authorization flow (checked live: protected-resource metadata and dynamic
 * client registration), except GitHub (a token) and Context7 (open).
 */
export type ConnectorCategory = "popular" | "developer" | "productivity";

export interface CatalogConnector {
  readonly id: string;
  readonly name: string;
  readonly url: string;
  readonly category: ConnectorCategory;
  readonly auth: ConnectorAuth;
  /** A raster mark from the brand's own site, when Simple Icons has none. */
  readonly image?: string;
  /** Supabase: a project ref and read-only mode go in the URL. */
  readonly supabase?: boolean;
}

export const CONNECTOR_CATALOG: readonly CatalogConnector[] = [
  { id: "supabase", name: "Supabase", url: "https://mcp.supabase.com/mcp", category: "popular", auth: "oauth", supabase: true },
  { id: "linear", name: "Linear", url: "https://mcp.linear.app/mcp", category: "popular", auth: "oauth" },
  { id: "notion", name: "Notion", url: "https://mcp.notion.com/mcp", category: "popular", auth: "oauth" },
  { id: "github", name: "GitHub", url: "https://api.githubcopilot.com/mcp/", category: "popular", auth: "token" },
  { id: "stripe", name: "Stripe", url: "https://mcp.stripe.com", category: "popular", auth: "oauth" },
  { id: "vercel", name: "Vercel", url: "https://mcp.vercel.com", category: "popular", auth: "oauth" },
  { id: "sentry", name: "Sentry", url: "https://mcp.sentry.dev/mcp", category: "popular", auth: "oauth" },
  { id: "cloudflare", name: "Cloudflare", url: "https://mcp.cloudflare.com/mcp", category: "developer", auth: "oauth" },
  { id: "neon", name: "Neon", url: "https://mcp.neon.tech/mcp", category: "developer", auth: "oauth" },
  { id: "posthog", name: "PostHog", url: "https://mcp.posthog.com/mcp", category: "developer", auth: "oauth" },
  { id: "netlify", name: "Netlify", url: "https://netlify-mcp.netlify.app/mcp", category: "developer", auth: "oauth" },
  { id: "railway", name: "Railway", url: "https://mcp.railway.com", category: "developer", auth: "oauth" },
  { id: "prisma", name: "Prisma", url: "https://mcp.prisma.io/mcp", category: "developer", auth: "oauth" },
  { id: "figma", name: "Figma", url: "https://mcp.figma.com/mcp", category: "developer", auth: "oauth" },
  { id: "huggingface", name: "Hugging Face", url: "https://huggingface.co/mcp", category: "developer", auth: "oauth" },
  { id: "context7", name: "Context7", url: "https://mcp.context7.com/mcp", category: "developer", auth: "none", image: context7Icon },
  { id: "atlassian", name: "Atlassian", url: "https://mcp.atlassian.com/v1/mcp", category: "productivity", auth: "oauth" },
  { id: "asana", name: "Asana", url: "https://mcp.asana.com/mcp", category: "productivity", auth: "oauth" },
  { id: "todoist", name: "Todoist", url: "https://ai.todoist.net/mcp", category: "productivity", auth: "oauth" },
  { id: "monday", name: "monday.com", url: "https://mcp.monday.com/mcp", category: "productivity", auth: "oauth", image: mondayIcon },
  { id: "canva", name: "Canva", url: "https://mcp.canva.com/mcp", category: "productivity", auth: "oauth", image: canvaIcon },
  { id: "intercom", name: "Intercom", url: "https://mcp.intercom.com/mcp", category: "productivity", auth: "oauth" },
  { id: "webflow", name: "Webflow", url: "https://mcp.webflow.com/mcp", category: "productivity", auth: "oauth" },
  { id: "wix", name: "Wix", url: "https://mcp.wix.com/mcp", category: "productivity", auth: "oauth" },
  { id: "paypal", name: "PayPal", url: "https://mcp.paypal.com/mcp", category: "productivity", auth: "oauth" },
  { id: "square", name: "Square", url: "https://mcp.squareup.com/mcp", category: "productivity", auth: "oauth" },
];

export const CONNECTOR_CATEGORIES: readonly ConnectorCategory[] = ["popular", "developer", "productivity"];


export const catalogEntry = (id: string | undefined) => CONNECTOR_CATALOG.find((entry) => entry.id === id);

/** The Supabase URL for one project and, optionally, read-only. */
export function supabaseUrl(base: string, projectRef: string, readOnly: boolean): string {
  const url = new URL(base);
  if (projectRef.trim()) url.searchParams.set("project_ref", projectRef.trim());
  if (readOnly) url.searchParams.set("read_only", "true");
  return url.toString();
}

/** What a connector's URL says about its Supabase project, for its row. */
export function supabaseDetails(url: string): { projectRef: string; readOnly: boolean } {
  try {
    const parsed = new URL(url);
    return { projectRef: parsed.searchParams.get("project_ref") ?? "", readOnly: parsed.searchParams.get("read_only") === "true" };
  } catch {
    return { projectRef: "", readOnly: false };
  }
}
