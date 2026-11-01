import { cx } from '../lib/cx';
import { baseName, truncateMiddle } from '../lib/format';
import type { Profile } from '../lib/types';
import { S } from '../strings';
import { Avatar } from './Avatar';
import { Button } from './Button';
import { Chip } from './Chip';
import { Ico, Icons } from './icons';
import { MenuButton } from './Menu';
import { Pill } from './Pill';

/**
 * Design §7.5. Only Edit and the overflow menu are interactive (the card itself
 * is not clickable, to avoid accidental edits).
 */
export function ProfileCard({
  profile,
  isDefault,
  testing = false,
  onEdit,
  onMakeDefault,
  onTest,
  onDelete,
}: {
  profile: Profile;
  isDefault: boolean;
  testing?: boolean;
  onEdit: () => void;
  onMakeDefault: () => void;
  onTest: () => void;
  onDelete: () => void;
}) {
  const titleId = `profile-${profile.id}-title`;
  const keyName = profile.ssh_key ? baseName(profile.ssh_key) : '';
  return (
    <article
      aria-labelledby={titleId}
      className={cx(
        'gh-card transition-colour-base rounded-lg bg-surface px-4 py-3',
        isDefault ? 'border-[1.5px] border-aqua' : 'border border-line hover:border-line-input',
      )}
    >
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <Avatar label={profile.label} color={profile.color} size={30} />
        <h2 id={titleId} className="min-w-0 truncate text-heading">
          {profile.label}
        </h2>
        {isDefault ? <Pill kind="ok">{S.pill.default}</Pill> : null}
        <div className="ml-auto flex items-center gap-1">
          {testing ? (
            <span className="flex items-center gap-1 text-small text-ink-muted" role="status">
              {S.profiles.testing}
            </span>
          ) : null}
          <Button
            icon={Icons.edit}
            onClick={onEdit}
            aria-label={S.profiles.editLabel(profile.label)}
          >
            {S.profiles.edit}
          </Button>
          <MenuButton
            iconOnly
            label={S.common.moreActions(profile.label)}
            trigger={<Ico icon={Icons.more} />}
            items={[
              ...(isDefault
                ? []
                : [
                    {
                      key: 'default',
                      label: S.profiles.makeDefault,
                      icon: Icons.ok,
                      onSelect: onMakeDefault,
                    },
                  ]),
              { key: 'test', label: S.profiles.testConnection, icon: Icons.test, onSelect: onTest },
              {
                key: 'delete',
                label: S.profiles.delete,
                icon: Icons.delete,
                danger: true,
                onSelect: onDelete,
              },
            ]}
          />
        </div>
      </div>
      <p
        className="mt-1 truncate pl-[2.8571rem] font-mono text-mono text-ink-muted"
        title={profile.email}
      >
        {profile.email}
      </p>
      {profile.owners.length || profile.folders.length || keyName ? (
        <ul className="mt-2 flex flex-wrap gap-1.5 pl-[2.8571rem]">
          {profile.owners.map((o) => (
            <li key={`o-${o}`} className="max-w-full">
              <Chip>{`github.com/${o}`}</Chip>
            </li>
          ))}
          {profile.folders.map((f) => (
            <li key={`f-${f}`} className="max-w-full">
              <Chip icon={Icons.folder} title={f}>
                {truncateMiddle(f, 28)}
              </Chip>
            </li>
          ))}
          {keyName ? (
            <li className="max-w-full">
              <Chip icon={Icons.key} title={profile.ssh_key}>
                {keyName}
              </Chip>
            </li>
          ) : null}
        </ul>
      ) : null}
    </article>
  );
}
