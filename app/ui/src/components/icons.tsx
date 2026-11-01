// Design §5.2 icon map, verified against @fluentui/react-icons 2.0.345.
// Differences from the map: WifiOff16Regular and Power16Regular don't exist —
// the 20 px glyphs are used and rendered at 16 px.
import type { FluentIcon } from '@fluentui/react-icons';
import {
  Add16Regular,
  ArrowExport16Regular,
  ArrowImport16Regular,
  ArrowUndo16Regular,
  Checkmark16Regular,
  Checkmark24Regular,
  ChevronDown16Regular,
  Copy16Regular,
  Delete16Regular,
  Dismiss12Regular,
  Dismiss16Regular,
  Edit16Regular,
  Eye16Regular,
  EyeOff16Regular,
  Folder16Regular,
  Folder20Filled,
  Folder20Regular,
  FolderSearch16Regular,
  FolderSearch24Regular,
  Info16Regular,
  Key16Regular,
  LockClosed16Regular,
  MoreHorizontal16Regular,
  Open16Regular,
  People20Filled,
  People20Regular,
  People24Regular,
  Pin16Regular,
  PlugConnected16Regular,
  Power20Regular,
  Settings20Filled,
  Settings20Regular,
  ShieldCheckmark16Regular,
  Warning16Regular,
  WifiOff20Regular,
  Window16Regular,
  Wrench20Filled,
  Wrench20Regular,
} from '@fluentui/react-icons';

export type Icon = FluentIcon;

export const Icons = {
  // nav (20 px, Filled when selected)
  profiles: People20Regular,
  profilesSelected: People20Filled,
  repos: Folder20Regular,
  reposSelected: Folder20Filled,
  fix: Wrench20Regular,
  fixSelected: Wrench20Filled,
  settings: Settings20Regular,
  settingsSelected: Settings20Filled,
  // inline (16 px)
  add: Add16Regular,
  edit: Edit16Regular,
  delete: Delete16Regular,
  key: Key16Regular,
  ok: Checkmark16Regular,
  blocked: LockClosed16Regular,
  warning: Warning16Regular,
  info: Info16Regular,
  scan: FolderSearch16Regular,
  pin: Pin16Regular,
  copy: Copy16Regular,
  external: Open16Regular,
  test: PlugConnected16Regular,
  export: ArrowExport16Regular,
  import: ArrowImport16Regular,
  undo: ArrowUndo16Regular,
  offline: WifiOff20Regular,
  shield: ShieldCheckmark16Regular,
  window: Window16Regular,
  quit: Power20Regular,
  more: MoreHorizontal16Regular,
  dismiss: Dismiss16Regular,
  dismissSmall: Dismiss12Regular,
  show: Eye16Regular,
  hide: EyeOff16Regular,
  folder: Folder16Regular,
  chevronDown: ChevronDown16Regular,
  // empty states (24 px)
  emptyProfiles: People24Regular,
  emptyScan: FolderSearch24Regular,
  emptyOk: Checkmark24Regular,
} satisfies Record<string, FluentIcon>;

/** Decorative icon: hidden from assistive tech, sized in px, inherits colour. */
export function Ico({
  icon: I,
  size = 16,
  className,
}: {
  icon: FluentIcon;
  size?: 12 | 16 | 20 | 24;
  className?: string;
}) {
  return (
    <I
      aria-hidden="true"
      focusable={false}
      className={['shrink-0', className].filter(Boolean).join(' ')}
      style={{ width: `${size / 14}rem`, height: `${size / 14}rem` }}
    />
  );
}
