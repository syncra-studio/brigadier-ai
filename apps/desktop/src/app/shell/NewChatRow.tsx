import { NavRow } from "@/components/controls";
import { ComposeEditSquare } from "@/components/icons";
import { select } from "@/state/actions";

/** The sidebar's New chat row, filled while a new chat is being written. */
export function NewChatRow({ active }: { active: boolean }) {
  return (
    <li>
      <NavRow
        data-active={active}
        onClick={() => select({ type: "draft", kind: "chat" })}
      >
        <ComposeEditSquare />
        <span className="truncate">New chat</span>
      </NavRow>
    </li>
  );
}
