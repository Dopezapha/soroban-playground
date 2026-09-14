"use client";

import React from "react";
import { useMonaco } from "@/hooks/useMonaco";
import { useCollaborativeEditor } from "@/hooks/useCollaborativeEditor";
import { CollaborativeHeaderIndicator } from "@/components/CollaborativeHeaderIndicator";

interface EditorProps {
  code: string;
  setCode: (value: string) => void;
}

function EditorLoadingState() {
  return (
    <div className="flex items-center justify-center h-full w-full text-gray-500">
      <div className="flex flex-col items-center gap-3">
        <div className="animate-spin rounded-full w-8 h-8 border-b-2 border-teal-500" />
        <span className="text-xs font-mono text-gray-400">
          Loading editor...
        </span>
      </div>
    </div>
  );
}

export default function Editor({ code, setCode }: EditorProps) {
  const { peers, isConnected } = useCollaborativeEditor();
  const { containerRef, isEditorReady } = useMonaco({
    language: "rust",
    value: code,
    onChange: setCode,
  });

  return (
    <div className="relative h-[500px] w-full rounded-xl overflow-hidden border border-gray-800 bg-[#1e1e1e] shadow-2xl flex flex-col">
      <div className="flex items-center justify-between px-4 py-2 border-b border-gray-800 bg-gray-900/90 text-xs text-gray-400">
        <span className="font-mono text-[11px] font-semibold tracking-wide text-slate-300">
          lib.rs (Soroban Smart Contract)
        </span>
        <CollaborativeHeaderIndicator peers={peers} isConnected={isConnected} />
      </div>
      <div className="flex-1 w-full relative">
        <div
          ref={containerRef}
          className="h-full w-full"
          data-testid="monaco-editor"
        />
        {!isEditorReady && <EditorLoadingState />}
      </div>
    </div>
  );
}