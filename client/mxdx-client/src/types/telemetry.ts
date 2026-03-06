import { z } from "zod";

export const CpuInfo = z.object({
  cores: z.number().int().positive(),
  usage_percent: z.number(),
});
export type CpuInfo = z.infer<typeof CpuInfo>;

export const MemoryInfo = z.object({
  total_bytes: z.number().int().nonnegative(),
  used_bytes: z.number().int().nonnegative(),
});
export type MemoryInfo = z.infer<typeof MemoryInfo>;

export const DiskInfo = z.object({
  total_bytes: z.number().int().nonnegative(),
  used_bytes: z.number().int().nonnegative(),
});
export type DiskInfo = z.infer<typeof DiskInfo>;

export const NetworkInfo = z.object({
  rx_bytes: z.number().int().nonnegative(),
  tx_bytes: z.number().int().nonnegative(),
});
export type NetworkInfo = z.infer<typeof NetworkInfo>;

export const HostTelemetryEvent = z.object({
  timestamp: z.string(),
  hostname: z.string(),
  os: z.string(),
  arch: z.string(),
  uptime_seconds: z.number().int().nonnegative(),
  load_avg: z.tuple([z.number(), z.number(), z.number()]),
  cpu: CpuInfo,
  memory: MemoryInfo,
  disk: DiskInfo,
  network: NetworkInfo.nullable().optional(),
  services: z.unknown().nullable().optional(),
  devices: z.unknown().nullable().optional(),
});
export type HostTelemetryEvent = z.infer<typeof HostTelemetryEvent>;
