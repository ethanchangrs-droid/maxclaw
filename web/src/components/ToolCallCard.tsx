import { useState } from 'react';
import { Wrench, CheckCircle, XCircle, ChevronDown, ChevronRight, Clock } from 'lucide-react';

interface ToolCallCardProps {
  type: 'tool_call' | 'tool_result';
  toolName?: string;
  toolArgs?: any;
  toolOutput?: string;
  toolSuccess?: boolean;
  toolDuration?: number;
  timestamp: Date;
}

export default function ToolCallCard({
  type,
  toolName,
  toolArgs,
  toolOutput,
  toolSuccess,
  toolDuration,
  timestamp,
}: ToolCallCardProps) {
  const [expanded, setExpanded] = useState(false);
  const isResult = type === 'tool_result';
  const name = toolName ?? 'unknown';

  return (
    <div className="flex items-start gap-3 animate-slide-in-left">
      <div
        className="flex-shrink-0 w-8 h-8 rounded-xl flex items-center justify-center"
        style={{ background: 'linear-gradient(135deg, #1a1a3e, #12122a)' }}
      >
        {isResult ? (
          toolSuccess !== false ? (
            <CheckCircle className="h-4 w-4 text-[#00e68a]" />
          ) : (
            <XCircle className="h-4 w-4 text-[#ff4466]" />
          )
        ) : (
          <Wrench className="h-4 w-4 text-[#ffaa33]" />
        )}
      </div>
      <div className="max-w-[75%]">
        <div
          className="rounded-xl px-3 py-2 border border-[#1a1a3e]/60 text-xs"
          style={{ background: 'linear-gradient(135deg, rgba(13,13,32,0.8), rgba(10,10,26,0.6))' }}
        >
          <div className="flex items-center gap-2">
            <span className="font-mono font-semibold text-[#0080ff]">{name}</span>
            {isResult && toolDuration != null && toolDuration > 0 && (
              <span className="flex items-center gap-0.5 text-[#556080]">
                <Clock className="h-3 w-3" />
                {toolDuration < 1000
                  ? `${toolDuration}ms`
                  : `${(toolDuration / 1000).toFixed(1)}s`}
              </span>
            )}
            {isResult && (
              <span
                className={`text-[10px] px-1.5 py-0.5 rounded-full ${
                  toolSuccess !== false
                    ? 'bg-[#00e68a15] text-[#00e68a]'
                    : 'bg-[#ff446615] text-[#ff4466]'
                }`}
              >
                {toolSuccess !== false ? 'ok' : 'error'}
              </span>
            )}
          </div>

          {!isResult && toolArgs && (
            <button
              onClick={() => setExpanded(!expanded)}
              className="flex items-center gap-1 mt-1.5 text-[#556080] hover:text-[#8888aa] transition-colors"
            >
              {expanded ? <ChevronDown className="h-3 w-3" /> : <ChevronRight className="h-3 w-3" />}
              <span>arguments</span>
            </button>
          )}
          {!isResult && expanded && toolArgs && (
            <pre className="mt-1 text-[#8888aa] bg-[#0a0a1a] rounded-lg p-2 overflow-x-auto max-h-[200px] overflow-y-auto">
              {JSON.stringify(toolArgs, null, 2)}
            </pre>
          )}

          {isResult && toolOutput && (
            <>
              <button
                onClick={() => setExpanded(!expanded)}
                className="flex items-center gap-1 mt-1.5 text-[#556080] hover:text-[#8888aa] transition-colors"
              >
                {expanded ? <ChevronDown className="h-3 w-3" /> : <ChevronRight className="h-3 w-3" />}
                <span>output</span>
              </button>
              {expanded && (
                <pre className="mt-1 text-[#8888aa] bg-[#0a0a1a] rounded-lg p-2 overflow-x-auto max-h-[200px] overflow-y-auto whitespace-pre-wrap break-words">
                  {toolOutput}
                </pre>
              )}
            </>
          )}
        </div>
        <p className="text-[10px] mt-1 text-[#334060] pl-1">
          {timestamp.toLocaleTimeString()}
        </p>
      </div>
    </div>
  );
}
