// Vitest environment: plain Node globals, but modules go through Vite's client pipeline so Svelte compiles runes for the
// browser (`$effect.root` and friends only run in the client build).
import type { Environment } from 'vitest/runtime';

const env: Environment = {
  name: 'node-client',
  viteEnvironment: 'client',
  setup() {
    return { teardown() {} };
  }
};
export default env;
