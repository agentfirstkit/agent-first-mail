import { bindings, defineConfig, exports, triggers } from "cf/config";

export default defineConfig({
  worker: {
    name: "__WORKER_NAME__",
    entrypoint: "src/index.ts",
    compatibilityDate: "2026-10-21",
    env: {
      MAIL_SPOOL: bindings.r2({ name: "__BUCKET_NAME__" }),
      MAIL_QUOTA: bindings.durableObject({ worker: "__WORKER_NAME__", exportName: "SpoolQuota" }),
      MAX_MESSAGE_BYTES: bindings.text("10485760"),
      MAX_SPOOL_BYTES: bindings.text("1073741824"),
      MAX_SPOOL_MESSAGES: bindings.text("10000"),
      RATE_WINDOW_S: bindings.text("60"),
      MAX_MESSAGES_PER_WINDOW: bindings.text("100"),
    },
    exports: { SpoolQuota: exports.durableObject({ storage: "sqlite" }) },
    // Replace this with your inbound addresses or a *@domain catch-all.
    // cf deploy manages these rules; do not also create the same rules manually.
    triggers: [triggers.email({ addresses: ["support@example.com"] })],
  },
});
