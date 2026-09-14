/** @type {import('jest').Config} */
const path = require("path");

// Absolute path to the frontend's own node_modules — used to resolve packages
// that are NOT hoisted to the workspace root (e.g. react, react-dom).
const frontendModules = path.resolve(__dirname, "node_modules");

// Absolute path to the workspace root node_modules — used to resolve packages
// that ARE hoisted (e.g. jest, jest-environment-jsdom, @testing-library/*).
const rootModules = path.resolve(__dirname, "../node_modules");

const fs = require("fs");

const reactDir = fs.existsSync(path.resolve(frontendModules, "react"))
  ? frontendModules
  : rootModules;

const config = {
  testEnvironment: "jsdom",
  setupFilesAfterEnv: ["<rootDir>/jest.setup.js"],
  moduleNameMapper: {
    "^@/(.*)$": "<rootDir>/src/$1",
    "\\.(css|less|scss|sass)$": "identity-obj-proxy",
    // Pin react and react-dom to resolved node_modules.
    "^react$": path.resolve(reactDir, "react"),
    "^react/(.*)$": path.resolve(reactDir, "react/$1"),
    "^react-dom$": path.resolve(reactDir, "react-dom"),
    "^react-dom/(.*)$": path.resolve(reactDir, "react-dom/$1"),
    "^monaco-editor$": path.resolve(
      rootModules,
      "monaco-editor/esm/vs/editor/editor.main.js"
    ),
    "^monaco-editor/(.*)$": path.resolve(rootModules, "monaco-editor/$1"),
  },
  // Resolve modules from both the frontend and the workspace root.
  modulePaths: [frontendModules, rootModules],
  transform: {
    "^.+\\.(ts|tsx)$": [
      "babel-jest",
      {
        presets: [
          ["@babel/preset-react", { runtime: "automatic" }],
          "@babel/preset-typescript",
        ],
        plugins: [
          "@babel/plugin-transform-modules-commonjs",
          "@babel/plugin-transform-dynamic-import",
          "babel-plugin-transform-import-meta",
        ],
      },
    ],
    "^.+\\.js$": [
      "babel-jest",
      {
        plugins: [
          "@babel/plugin-transform-modules-commonjs",
          "@babel/plugin-transform-dynamic-import",
          "babel-plugin-transform-import-meta",
        ],
      },
    ],
  },
  transformIgnorePatterns: ["/node_modules/(?!(lucide-react|monaco-editor)/)"],
};

module.exports = config;
