import { z } from "zod";

export const LauncherIdentityEvent = z.object({
  launcher_id: z.string(),
  accounts: z.array(z.string()),
  primary: z.string(),
  capabilities: z.array(z.string()),
  version: z.string(),
});
export type LauncherIdentityEvent = z.infer<typeof LauncherIdentityEvent>;
