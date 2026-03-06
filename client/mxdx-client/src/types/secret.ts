import { z } from "zod";

export const SecretRequestEvent = z.object({
  request_id: z.string(),
  scope: z.string(),
  ttl_seconds: z.number().int().nonnegative(),
  reason: z.string(),
  ephemeral_public_key: z.string(),
});
export type SecretRequestEvent = z.infer<typeof SecretRequestEvent>;

export const SecretResponseEvent = z.object({
  request_id: z.string(),
  granted: z.boolean(),
  encrypted_value: z.string().nullable().optional(),
  error: z.string().nullable().optional(),
});
export type SecretResponseEvent = z.infer<typeof SecretResponseEvent>;
