// Объявления web/cloud/dpop.ts (файл скрытой части на заглушках) для tsc основы. Пишет scripts/t3-sync.py.
import { DpopProof } from "@t3tools/client-runtime/relay";
import { DpopPublicJwk } from "@t3tools/shared/dpop";
import * as Digest from "effect/crypto/Digest";
import * as SecureRandom from "effect/crypto/SecureRandom";
import * as Signature from "effect/crypto/Signature";
import * as Effect from "effect/Effect";
import * as Layer from "effect/Layer";
import * as Schema from "effect/Schema";
export interface BrowserDpopKey {
    readonly privateKey: CryptoKey;
    readonly publicJwk: DpopPublicJwk;
    readonly thumbprint: string;
}
declare const BrowserDpopError_base: Schema.Class<BrowserDpopError, Schema.TaggedStruct<"BrowserDpopError", {
    readonly message: Schema.String;
    readonly cause: Schema.optional<Schema.Defect>;
}>, import("effect/Cause").YieldableError>;
export declare class BrowserDpopError extends BrowserDpopError_base {
}
export declare const layer: Layer.Layer<Signature.Signature | Digest.Digest | SecureRandom.SecureRandom, never, never>;
/** The stored key, or a new one persisted on first use, as a DPoP proof key. */
export declare const loadOrCreateProofKey: () => Effect.Effect<{
    publicJwk: {
        readonly kty: "EC";
        readonly crv: "P-256";
        readonly x: string;
        readonly y: string;
    };
    sign: (signingInput: Uint8Array<ArrayBufferLike>) => Effect.Effect<Uint8Array<ArrayBufferLike>, DpopProof.DpopProofError, never>;
}, BrowserDpopError, Signature.Signature>;
export {};
