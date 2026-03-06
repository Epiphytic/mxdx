import { z } from "zod";

export const ResultStatus = z.enum(["exit", "error", "timeout", "killed"]);
export type ResultStatus = z.infer<typeof ResultStatus>;

export const ResultEvent = z.object({
  uuid: z.string(),
  status: ResultStatus,
  exit_code: z.number().int().nullable().optional(),
  summary: z.string().nullable().optional(),
});
export type ResultEvent = z.infer<typeof ResultEvent>;
