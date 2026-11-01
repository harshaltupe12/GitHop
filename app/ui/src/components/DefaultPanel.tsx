import { sortProfiles } from '../lib/format';
import type { ProfileSet } from '../lib/types';
import { S } from '../strings';
import { Avatar } from './Avatar';
import { MenuButton } from './Menu';

/**
 * Design §7.3 — bottom of the sidebar: caption "Default", colour dot + label.
 * Click opens a menu to change the default profile (same as the tray menu).
 */
export function DefaultPanel({
  profiles,
  onChange,
}: {
  profiles: ProfileSet;
  onChange: (id: string) => void;
}) {
  const list = sortProfiles(profiles);
  const current = list.find((p) => p.id === profiles.default);
  const name = current?.label ?? S.defaultPanel.none;
  return (
    <MenuButton
      label={S.defaultPanel.change(name)}
      heading={S.defaultPanel.menuLabel}
      radio
      align="left"
      placement="top"
      triggerClassName="block w-full rounded-md bg-bg p-2 text-left hover:bg-surface-hover"
      items={list.map((p) => ({
        key: p.id,
        label: p.label,
        text: p.label,
        checked: p.id === profiles.default,
        leading: <Avatar label={p.label} color={p.color} size={22} decorative />,
        onSelect: () => onChange(p.id),
      }))}
      trigger={
        <span className="flex flex-col gap-0.5">
          <span className="text-small text-ink-muted">{S.defaultPanel.caption}</span>
          <span className="flex min-w-0 items-center gap-2 text-body font-medium">
            {current ? (
              <Avatar label={current.label} color={current.color} size={8} decorative />
            ) : null}
            <span className="truncate">{name}</span>
          </span>
        </span>
      }
    />
  );
}
