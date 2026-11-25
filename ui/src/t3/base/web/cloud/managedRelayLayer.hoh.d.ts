// Объявления web/cloud/managedRelayLayer.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import { ManagedRelay } from "@t3tools/client-runtime/relay";
import * as Layer from "effect/Layer";
export declare const layer: (relayUrl: string) => Layer.Layer<ManagedRelay.ManagedRelayDpopSigner | ManagedRelay.ManagedRelayClient, never, import("effect/crypto/Signature").Signature | import("effect/http/HttpClient").HttpClient | import("effect/crypto/Digest").Digest | import("effect/crypto/SecureRandom").SecureRandom>;
