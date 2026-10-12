import { SimpleSmile } from "@openai/apps-sdk-ui/components/Icon";
import { useState } from "react";

import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";

const EMOJI = [
  "📝", "📄", "📌", "📎", "📚", "📖", "🗒️", "🗂️",
  "💡", "✅", "☑️", "⭐", "🔥", "🚀", "🎯", "🏁",
  "🐛", "🔧", "🛠️", "⚙️", "🧪", "🧭", "🧩", "🔒",
  "📈", "📊", "🗓️", "⏰", "💬", "❓", "❗", "⚠️",
  "🎨", "🖼️", "🎵", "🌱", "🌍", "🏠", "❤️", "🙂",
];

/** The page's icon left of its title: a smile to add one, the emoji once chosen. */
export function EmojiButton({ emoji, onChange }: { emoji: string | null; onChange: (emoji: string | null) => void }) {
  const [open, setOpen] = useState(false);
  const pick = (next: string | null) => { onChange(next); setOpen(false); };
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <TooltipIconButton tooltip={emoji ? "Change icon" : "Add icon"} size="icon-md" data-chosen={emoji ? true : undefined} className="page-emoji-button">
          {emoji ? <span aria-hidden className="page-emoji">{emoji}</span> : <SimpleSmile />}
        </TooltipIconButton>
      </PopoverTrigger>
      <PopoverContent align="start" className="page-emoji-picker">
        <div role="listbox" aria-label="Icons" className="page-emoji-grid">
          {EMOJI.map((choice) => (
            <button key={choice} type="button" role="option" aria-selected={choice === emoji} aria-label={choice}
              className="page-emoji-choice" onClick={() => pick(choice)}>{choice}</button>
          ))}
        </div>
        {emoji && <Button size="sm" variant="ghost" className="w-full" onClick={() => pick(null)}>Remove icon</Button>}
      </PopoverContent>
    </Popover>
  );
}
