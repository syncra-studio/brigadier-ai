import { useState } from "react";

import { Popover, PopoverContent, PopoverTrigger } from "@/components/overlays";
import { type MockupNote, notesByScreen } from "@/state/mockupNotes";

/** "{n} comments"; hovering lists them per image, as they will be sent. */
export function NotesList({ notes }: { notes: MockupNote[] }) {
  const [open, setOpen] = useState(false);
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button type="button" className="text-muted-foreground hover:text-foreground px-2.5" onPointerEnter={() => setOpen(true)} onPointerLeave={() => setOpen(false)}>
          {notes.length === 1 ? "1 comment" : `${notes.length} comments`}
        </button>
      </PopoverTrigger>
      <PopoverContent side="top" className="w-mockup-list flex flex-col gap-3 p-3 text-sm" onOpenAutoFocus={(event) => event.preventDefault()}>
        {notesByScreen(notes).map(({ screen, notes: own }, index) => (
          <div key={screen} className="flex flex-col gap-2">
            <span className="font-medium">Image {index + 1}</span>
            <ol className="flex flex-col gap-2">
              {own.map((note, number) => (
                <li key={note.id} className="flex gap-2"><span className="text-muted-foreground">{number + 1}.</span>{note.text}</li>
              ))}
            </ol>
          </div>
        ))}
      </PopoverContent>
    </Popover>
  );
}
