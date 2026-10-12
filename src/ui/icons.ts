const paths = {
  wave: '<path d="M4 10v4m4-8v12m4-15v18m4-15v12m4-8v4"/>',
  browser:
    '<rect x="3" y="4" width="18" height="16" rx="3"/><path d="M3 9h18M7 6.5h.01M10 6.5h.01"/>',
  leaf: '<path d="M20 4c-8-2-16 2-15 9 1 6 9 7 12 2 2-3 1-7 3-11Z"/><path d="m4 21 10-11"/>',
  arrow: '<path d="M5 12h14m-6-6 6 6-6 6"/>',
  external:
    '<path d="M14 3h7v7m0-7L10 14M10 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-5"/>',
  search: '<circle cx="10.5" cy="10.5" r="6.5"/><path d="m16 16 5 5"/>',
  back: '<path d="m14 6-6 6 6 6"/>',
  forward: '<path d="m10 6 6 6-6 6"/>',
  reload: '<path d="M20 7v5h-5m5 0a8 8 0 1 0-2 6M20 12a8 8 0 0 0-2-6"/>',
  home: '<path d="m3 10 9-7 9 7v10H3Zm6 10v-7h6v7"/>',
  lock: '<rect x="5" y="10" width="14" height="11" rx="2"/><path d="M8 10V7a4 4 0 0 1 8 0v3m-4 4v3"/>',
  music:
    '<path d="M9 18V5l12-2v13M9 8l12-2"/><ellipse cx="6" cy="18" rx="3" ry="3"/><ellipse cx="18" cy="16" rx="3" ry="3"/>',
  play: '<path d="m9 5 11 7-11 7Z"/>',
  previous: '<path d="M5 5v14m14-14L8 12l11 7Z"/>',
  next: '<path d="M19 5v14M5 5l11 7-11 7Z"/>',
};

export function icon(name: keyof typeof paths, className = ""): string {
  return `<svg class="icon ${className}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${paths[name]}</svg>`;
}
