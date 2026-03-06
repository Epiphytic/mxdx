import { z } from "zod";

export const CommandAction = z.enum(["exec", "kill", "signal"]);
export type CommandAction = z.infer<typeof CommandAction>;

export const CommandEvent = z.object({
  uuid: z.string(),
  action: CommandAction,
  cmd: z.string(),
  args: z.array(z.string()),
  env: z.record(z.string(), z.string()),
  cwd: z.string().nullable().optional(),
  timeout_seconds: z.number().int().nonnegative().nullable().optional(),
});
export type CommandEvent = z.infer<typeof CommandEvent>;
