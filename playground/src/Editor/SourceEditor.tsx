import MonacoEditor, { type Monaco } from "@monaco-editor/react";

interface Props {
  source: string;
  filename: string;
  onChange: (value: string) => void;
}

function defineTheme(monaco: Monaco) {
  monaco.editor.defineTheme("tryke", {
    base: "vs-dark",
    inherit: true,
    rules: [
      { token: "comment", foreground: "6c7086", fontStyle: "italic" },
      { token: "keyword", foreground: "cba6f7" },
      { token: "string", foreground: "a6e3a1" },
      { token: "number", foreground: "fab387" },
      { token: "type", foreground: "89b4fa" },
      { token: "function", foreground: "89b4fa" },
      { token: "variable", foreground: "cdd6f4" },
      { token: "operator", foreground: "94e2d5" },
      { token: "decorator", foreground: "f9e2af" },
      { token: "delimiter", foreground: "9399b2" },
    ],
    colors: {
      "editor.background": "#1e1e2e",
      "editor.foreground": "#cdd6f4",
      "editor.lineHighlightBackground": "#31324420",
      "editor.selectionBackground": "#45475a80",
      "editor.inactiveSelectionBackground": "#45475a40",
      "editorLineNumber.foreground": "#6c7086",
      "editorLineNumber.activeForeground": "#a6adc8",
      "editorCursor.foreground": "#f5e0dc",
      "editorWhitespace.foreground": "#31324440",
      "editorIndentGuide.background": "#31324440",
      "editorIndentGuide.activeBackground": "#45475a",
      "editor.findMatchBackground": "#f9e2af30",
      "editor.findMatchHighlightBackground": "#f9e2af20",
      "editorWidget.background": "#181825",
      "editorWidget.border": "#313244",
      "input.background": "#1e1e2e",
      "input.border": "#313244",
      "input.foreground": "#cdd6f4",
      "scrollbar.shadow": "#00000000",
      "scrollbarSlider.background": "#45475a40",
      "scrollbarSlider.hoverBackground": "#45475a80",
      "scrollbarSlider.activeBackground": "#45475aaa",
    },
  });
}

export function SourceEditor({ source, filename, onChange }: Props) {
  return (
    <MonacoEditor
      language="python"
      theme="tryke"
      value={source}
      path={filename}
      onChange={(v) => onChange(v ?? "")}
      beforeMount={defineTheme}
      options={{
        minimap: { enabled: false },
        fontSize: 14,
        lineNumbers: "on",
        scrollBeyondLastLine: false,
        automaticLayout: true,
        padding: { top: 12 },
        tabSize: 4,
        insertSpaces: true,
      }}
    />
  );
}
