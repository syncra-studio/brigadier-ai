import { useEffect } from "react";
import { createRoot } from "react-dom/client";

import { Badge } from "@/components/ui/badge";

function Fixture() {
  useEffect(() => {
    const badge = document.querySelector('[data-slot="badge"]')!;
    const probe = document.getElementById("padding-token")!;
    const style = getComputedStyle(badge);
    const sizes = {
      token: parseFloat(getComputedStyle(probe).width),
      left: parseFloat(style.paddingLeft),
      right: parseFloat(style.paddingRight),
      height: parseFloat(style.height),
    };
    const result = document.createElement("pre");
    result.id = "badge-result";
    result.textContent = JSON.stringify(sizes);
    document.body.append(result);
    return () => result.remove();
  }, []);
  return <>
    <Badge variant="warning">Needs you</Badge>
    <div id="padding-token" style={{ width: "var(--spacing-pill-x)" }} />
  </>;
}

createRoot(document.getElementById("root")!).render(<Fixture />);
