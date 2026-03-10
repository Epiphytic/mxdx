import { z } from 'zod';

export const TerminalDataEvent = z.object({
  data: z.string(),
  encoding: z.string(),
  seq: z.number().int().nonnegative(),
  session_id: z.string().optional(),
});

export const TerminalResizeEvent = z.object({
  cols: z.number().int().nonnegative(),
  rows: z.number().int().nonnegative(),
  session_id: z.string().optional(),
});

export const TerminalSessionRequestEvent = z.object({
  uuid: z.string(),
  command: z.string(),
  env: z.record(z.string(), z.string()),
  cols: z.number().int().nonnegative(),
  rows: z.number().int().nonnegative(),
});

export const TerminalSessionResponseEvent = z.object({
  uuid: z.string(),
  status: z.string(),
  room_id: z.string().nullable().optional(),
});

export const TerminalRetransmitEvent = z.object({
  from_seq: z.number().int().nonnegative(),
  to_seq: z.number().int().nonnegative(),
});
