/** Inline 24-grid stroke icons. No icon font, no dependency: every glyph is a
 *  path so it inherits `currentColor` and the 1.6 stroke weight of the material. */

import type { CSSProperties } from 'react';

export type IconName =
  | 'boards'
  | 'layers'
  | 'assets'
  | 'templates'
  | 'style'
  | 'layout'
  | 'check'
  | 'undo'
  | 'redo'
  | 'zoomIn'
  | 'zoomOut'
  | 'fit'
  | 'export'
  | 'save'
  | 'eye'
  | 'eyeOff'
  | 'lock'
  | 'unlock'
  | 'plus'
  | 'close'
  | 'copy'
  | 'trash'
  | 'chevron'
  | 'chevronDown'
  | 'text'
  | 'image'
  | 'square'
  | 'circle'
  | 'line'
  | 'group'
  | 'send'
  | 'stop'
  | 'info'
  | 'question'
  | 'settings'
  | 'alignL'
  | 'alignC'
  | 'alignR'
  | 'alignT'
  | 'alignM'
  | 'alignB'
  | 'folder'
  | 'refresh'
  | 'sparkle'
  | 'link'
  | 'code'
  | 'sliders'
  | 'crop'
  | 'pin'
  | 'history'
  | 'minimize'
  | 'maximize'
  | 'restore'
  | 'grip'
  | 'wand'
  | 'swap'
  | 'bolt'
  | 'down'
  | 'up'
  | 'front'
  | 'back';

const P: Record<IconName, string[]> = {
  boards: ['M4 4h11v11H4z', 'M18 8v12H8'],
  layers: ['M12 3 3 8l9 5 9-5-9-5z', 'M3 14l9 5 9-5'],
  assets: ['M4 5h16v14H4z', 'M4 15l5-5 4 4 3-3 4 4', 'M9 9.5a1.2 1.2 0 1 0 0-.01'],
  templates: ['M4 4h7v7H4z', 'M13 4h7v7h-7z', 'M4 13h7v7H4z', 'M13 13h7v7h-7z'],
  style: ['M12 3c4 4 6 6.5 6 9a6 6 0 0 1-12 0c0-2.5 2-5 6-9z'],
  layout: ['M4 4h16v16H4z', 'M4 10h16', 'M11 10v10'],
  check: ['M4 12.5 9 17.5 20 6.5'],
  undo: ['M9 8H5V4', 'M5 8c3-4 9-4 12-1s3 8-1 11-10 2-12-1'],
  redo: ['M15 8h4V4', 'M19 8c-3-4-9-4-12-1s-3 8 1 11 10 2 12-1'],
  zoomIn: ['M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14z', 'M16.5 16.5 21 21', 'M11 8v6', 'M8 11h6'],
  zoomOut: ['M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14z', 'M16.5 16.5 21 21', 'M8 11h6'],
  fit: ['M4 9V4h5', 'M20 9V4h-5', 'M4 15v5h5', 'M20 15v5h-5'],
  export: ['M12 16V4', 'M8 8l4-4 4 4', 'M4 16v4h16v-4'],
  save: ['M5 4h11l3 3v13H5z', 'M9 4v6h6V4', 'M9 14h6v7H9z'],
  eye: ['M2 12s4-6 10-6 10 6 10 6-4 6-10 6S2 12 2 12z', 'M12 9.5a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5z'],
  eyeOff: ['M4 4l16 16', 'M9.5 5.5A9.5 9.5 0 0 1 22 12a15 15 0 0 1-3.5 4.3', 'M6.6 7.2A14.6 14.6 0 0 0 2 12s4 6 10 6a10 10 0 0 0 3-.5'],
  lock: ['M7 11h10v8H7z', 'M9.5 11V8a2.5 2.5 0 0 1 5 0v3'],
  unlock: ['M7 11h10v8H7z', 'M9.5 11V8a2.5 2.5 0 0 1 4.9-.6'],
  plus: ['M12 5v14', 'M5 12h14'],
  close: ['M6 6l12 12', 'M18 6 6 18'],
  copy: ['M8 8h11v11H8z', 'M5 16V5h11'],
  trash: ['M5 7h14', 'M9 7V4h6v3', 'M7 7l1 13h8l1-13', 'M11 11v6'],
  chevron: ['M9 6l6 6-6 6'],
  chevronDown: ['M6 9l6 6 6-6'],
  text: ['M5 6h14', 'M12 6v13', 'M9 19h6'],
  image: ['M4 5h16v14H4z', 'M4 15l5-4 4 3 3-3 4 4', 'M15 9a1.4 1.4 0 1 0 0-.01'],
  square: ['M5 5h14v14H5z'],
  circle: ['M12 5a7 7 0 1 0 0 14 7 7 0 0 0 0-14z'],
  line: ['M5 19 19 5'],
  group: ['M4 4h7v7H4z', 'M13 13h7v7h-7z', 'M13 4h7v7h-7z', 'M4 13h7v7H4z'],
  send: ['M4 12l16-7-6 16-3-6-7-3z'],
  stop: ['M6 6h12v12H6z'],
  info: ['M12 4a8 8 0 1 0 0 16 8 8 0 0 0 0-16z', 'M12 11v6', 'M12 8v.01'],
  question: ['M12 4a8 8 0 1 0 0 16 8 8 0 0 0 0-16z', 'M9.5 9.5A2.6 2.6 0 0 1 14.6 10c0 1.8-2.6 2-2.6 4', 'M12 17v.01'],
  settings: ['M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6z', 'M12 3v3', 'M12 18v3', 'M3 12h3', 'M18 12h3', 'M5.6 5.6 7.8 7.8', 'M16.2 16.2l2.2 2.2', 'M18.4 5.6l-2.2 2.2', 'M7.8 16.2l-2.2 2.2'],
  alignL: ['M4 4v16', 'M8 8h9', 'M8 15h6'],
  alignC: ['M12 4v16', 'M7 8h10', 'M9 15h6'],
  alignR: ['M20 4v16', 'M11 8h9', 'M14 15h6'],
  alignT: ['M4 4h16', 'M8 8v9', 'M15 8v6'],
  alignM: ['M4 12h16', 'M8 7v9', 'M15 9v6'],
  alignB: ['M4 20h16', 'M8 15V8', 'M15 15v-6'],
  folder: ['M4 6h6l2 2h8v10H4z'],
  refresh: ['M20 12a8 8 0 1 1-3-5.8', 'M20 4v4h-4'],
  sparkle: ['M12 4l1.8 4.7L18.5 10l-4.7 1.8L12 16l-1.8-4.2L5.5 10l4.7-1.3z', 'M18 16l.9 2.1 2.1.9-2.1.9-.9 2.1-.9-2.1-2.1-.9 2.1-.9z'],
  link: ['M10 14a4 4 0 0 1 0-5.7l2-2a4 4 0 0 1 5.7 5.7l-1 1', 'M14 10a4 4 0 0 1 0 5.7l-2 2A4 4 0 0 1 6.3 12l1-1'],
  code: ['M9 7 4 12l5 5', 'M15 7l5 5-5 5'],
  sliders: ['M4 8h10', 'M18 8h2', 'M4 16h6', 'M14 16h6', 'M15 5v6', 'M11 13v6'],
  crop: ['M7 3v14h14', 'M3 7h14v14'],
  pin: ['M12 4v9', 'M8 13h8l-2 7h-4z', 'M9 4h6'],
  history: ['M4 12a8 8 0 1 0 8-8 8 8 0 0 0-6.7 3.6', 'M12 8v5l4 2', 'M4 4v4h4'],
  minimize: ['M6 12h12'],
  maximize: ['M5 5h14v14H5z'],
  restore: ['M8 8h11v11H8z', 'M5 16V5h11'],
  grip: ['M4 14h16', 'M4 10h16'],
  wand: ['M4 20 15 9', 'M13 4l1 3 3 1-3 1-1 3-1-3-3-1 3-1z', 'M19 12l.7 1.8 1.8.7-1.8.7-.7 1.8-.7-1.8-1.8-.7 1.8-.7z'],
  swap: ['M4 8h13l-3-3', 'M20 16H7l3 3'],
  bolt: ['M13 3 6 14h5l-1 7 7-11h-5z'],
  down: ['M12 5v14', 'M6 13l6 6 6-6'],
  up: ['M12 19V5', 'M6 11l6-6 6 6'],
  front: ['M5 5h8v8H5z', 'M11 11h8v8h-8z'],
  back: ['M11 5H3v8h8z', 'M13 11h8v8h-8z'],
};

export interface IconProps {
  name: IconName;
  size?: number;
  className?: string;
  style?: CSSProperties;
  sw?: number;
}

export function Icon({ name, size = 16, className, style, sw = 1.6 }: IconProps) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={sw}
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      style={style}
      aria-hidden="true"
      focusable="false"
    >
      {P[name].map((d, i) => (
        <path key={i} d={d} />
      ))}
    </svg>
  );
}
