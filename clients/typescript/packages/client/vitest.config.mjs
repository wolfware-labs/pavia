import { fileURLToPath, URL } from "node:url";
import { defineProject } from "vitest/config";

export default defineProject({
  resolve: {
    alias: { "@pavia/client": fileURLToPath(new URL("./src/index.ts", import.meta.url)) },
  },
});
