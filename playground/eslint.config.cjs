const js = require("@eslint/js");
const tseslint = require("typescript-eslint");
const reactHooks = require("eslint-plugin-react-hooks");

// CommonJS flat config so the pre-commit `eslint` hook can resolve its plugins
// from prek's hermetic node env via NODE_PATH (ESM imports ignore NODE_PATH;
// require() honors it). ESLint auto-discovers this file for local `eslint src/`.
module.exports = tseslint.config(
  { ignores: ["dist/", "src/wasm/"] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    plugins: { "react-hooks": reactHooks },
    rules: {
      ...reactHooks.configs.recommended.rules,
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_" },
      ],
    },
  },
);
