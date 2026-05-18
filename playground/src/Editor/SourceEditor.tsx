import MonacoEditor from "@monaco-editor/react";

interface Props {
  source: string;
  filename: string;
  onChange: (value: string) => void;
}

export function SourceEditor({ source, filename, onChange }: Props) {
  return (
    <MonacoEditor
      language="python"
      theme="vs-dark"
      value={source}
      path={filename}
      onChange={(v) => onChange(v ?? "")}
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
