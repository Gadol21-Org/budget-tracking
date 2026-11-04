import { useLayoutEffect, useRef } from "react";

/**
 * Place a piece of the not yet converted (DOM-building) code inside a React tree. `build` runs once, when
 * the component is first shown. Goes away with the screens that still build their own DOM.
 */
export function Embed({ build }: { build: () => Node | null }) {
  const host = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const node = build();
    if (node) host.current?.replaceChildren(node);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- once, when first shown
  }, []);
  return <div ref={host} style={{ display: "contents" }} />;
}
