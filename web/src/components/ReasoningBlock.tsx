import { useState } from 'react';
import { ChevronDown, ChevronRight, Brain } from 'lucide-react';

interface ReasoningBlockProps {
  content: string;
  timestamp: Date;
}

export default function ReasoningBlock({ content, timestamp }: ReasoningBlockProps) {
  const [expanded, setExpanded] = useState(false);

  return (
    <div className="flex items-start gap-3 animate-slide-in-left">
      <div
        className="flex-shrink-0 w-8 h-8 rounded-xl flex items-center justify-center"
        style={{ background: 'linear-gradient(135deg, #1a1a3e, #12122a)' }}
      >
        <Brain className="h-4 w-4 text-[#8888cc]" />
      </div>
      <div className="max-w-[75%]">
        <button
          onClick={() => setExpanded(!expanded)}
          className="flex items-center gap-2 rounded-xl px-3 py-2 border border-[#1a1a3e]/60 text-[#8888aa] hover:text-[#aaaacc] hover:border-[#2a2a4e] transition-colors duration-200 text-xs"
          style={{ background: 'linear-gradient(135deg, rgba(15,15,36,0.9), rgba(10,10,28,0.7))' }}
        >
          {expanded ? (
            <ChevronDown className="h-3 w-3 flex-shrink-0" />
          ) : (
            <ChevronRight className="h-3 w-3 flex-shrink-0" />
          )}
          <span className="font-medium">Thinking</span>
          {!expanded && (
            <span className="text-[#556080] truncate max-w-[300px]">
              {content.slice(0, 80)}{content.length > 80 ? '...' : ''}
            </span>
          )}
        </button>
        {expanded && (
          <div
            className="mt-1 rounded-xl px-4 py-3 border border-[#1a1a3e]/40 text-[#8888aa] text-xs leading-relaxed whitespace-pre-wrap break-words"
            style={{ background: 'linear-gradient(135deg, rgba(10,10,28,0.6), rgba(8,8,22,0.4))' }}
          >
            {content}
          </div>
        )}
        <p className="text-[10px] mt-1 text-[#334060] pl-1">
          {timestamp.toLocaleTimeString()}
        </p>
      </div>
    </div>
  );
}
