import { z } from "zod";

export const TerminalDataEvent = z.object({
  data: z.string(),
  encoding: z.string(),
  seq: z.number().int().nonnegative(),
});
export type TerminalDataEvent = z.infer<typeof TerminalDataEvent>;

export const TerminalResizeEvent = z.object({
  cols: z.number().int().nonnegative(),
  rows: z.number().int().nonnegative(),
});
export type TerminalResizeEvent = z.infer<typeof TerminalResizeEvent>;

export const TerminalSessionRequestEvent = z.object({
  uuid: z.string(),
  command: z.string(),
  env: z.record(z.string(), z.string()),
  cols: z.number().int().nonnegative(),
  rows: z.number().int().nonnegative(),
});
export type TerminalSessionRequestEvent = z.infer<typeof TerminalSessionRequestEvent>;

export const TerminalSessionResponseEvent = z.object({
  uuid: z.string(),
  status: z.string(),
  room_id: z.string().nullable().optional(),
});
export type TerminalSessionResponseEvent = z.infer<typeof TerminalSessionResponseEvent>;

export const TerminalRetransmitEvent = z.object({
  from_seq: z.number().int().nonnegative(),
  to_seq: z.number().int().nonnegative(),
});
export type TerminalRetransmitEvent = z.infer<typeof TerminalRetransmitEvent>;
