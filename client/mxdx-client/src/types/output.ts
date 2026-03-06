import { z } from "zod";

export const OutputStream = z.enum(["stdout", "stderr"]);
export type OutputStream = z.infer<typeof OutputStream>;

export const OutputEvent = z.object({
  uuid: z.string(),
  stream: OutputStream,
  data: z.string(),
  encoding: z.string(),
  seq: z.number().int().nonnegative(),
});
export type OutputEvent = z.infer<typeof OutputEvent>;
