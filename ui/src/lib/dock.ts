/** Moves a bar into `<main>`'s strip below the scrolling page. Alone (in a test) it stays in place. */
export function dock(node: HTMLElement): { destroy(): void } {
  document.getElementById('page-dock')?.append(node)
  return { destroy: () => node.remove() }
}
