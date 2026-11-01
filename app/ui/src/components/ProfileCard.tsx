import { useConnectionEntry } from '../lib/connection';
import { cx } from '../lib/cx';
import { baseName, truncateMiddle } from '../lib/format';
import type { Profile, ValidationIssue } from '../lib/types';
import { S } from '../strings';
import { Avatar } from './Avatar';
import { Button } from './Button';
import { Chip } from './Chip';
import { ConnectionResult } from './ConnectionResult';
import { Ico, Icons } from './icons';
import { MenuButton } from './Menu';
import { Pill } from './Pill';

/**
 * Design §7.5. Only Edit, the overflow menu and a warning's own action are
 * interactive (the card itself is not clickable, to avoid accidental edits).
 * Below the chips: the core's warnings for this profile (icon + text, with
 * "Add owner" for the no-owners warning) and the last "Test connection" result.
 */
export function ProfileCard({
  profile,
  isDefault,
  issues = [],
  onEdit,
  onAddOwner,
  onMakeDefault,
  onTest,
  onDelete,
}: {
  profile: Profile;
  isDefault: boolean;
  /** Core validation issues for this profile (warnings shown; info ignored). */
  issues?: ValidationIssue[];
  onEdit: () => void;
  /** Opens Edit with the Owns field focused. */
  onAddOwner?: () => void;
  onMakeDefault: () => void;
  onTest: () => void;
  onDelete: () => void;
}) {
  const titleId = `profile-${profile.id}-title`;
  const keyName = profile.ssh_key ? baseName(profile.ssh_key) : '';
  const testing = useConnectionEntry(profile.id)?.status === 'testing';
  const shown = issues.filter((i) => i.severity !== 'info');
  const pad = 'pl-[2.8571rem]';
  return (
    <article
      aria-labelledby={titleId}
      aria-busy={testing || undefined}
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
              {
                key: 'test',
                label: testing ? S.profiles.testing : S.profiles.testConnection,
                icon: Icons.test,
                disabled: testing,
                onSelect: onTest,
              },
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
      <p className={cx('mt-1 truncate font-mono text-mono text-ink-muted', pad)} title={profile.email}>
        {profile.email}
      </p>
      {profile.owners.length || profile.folders.length || keyName ? (
        <ul className={cx('mt-2 flex flex-wrap gap-1.5', pad)}>
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
      {shown.length ? (
        <ul className={cx('mt-2 flex flex-col gap-1.5', pad)} aria-label={S.profiles.warningsLabel(profile.label)}>
          {shown.map((i, n) => (
            <li
              key={`${i.field}-${n}`}
              className={cx(
                'flex flex-wrap items-start gap-x-2 gap-y-1 text-small',
                i.severity === 'error' ? 'text-blocked-text' : 'text-warning-ink',
              )}
            >
              <span className="flex min-w-0 flex-1 items-start gap-1">
                <Ico icon={Icons.warning} className="mt-px" />
                <span className="min-w-[12rem] flex-1">{i.message}</span>
              </span>
              {i.field === 'owners' && onAddOwner ? (
                <Button
                  compact
                  icon={Icons.add}
                  onClick={onAddOwner}
                  aria-label={S.profiles.addOwnerLabel(profile.label)}
                >
                  {S.profiles.addOwner}
                </Button>
              ) : null}
            </li>
          ))}
        </ul>
      ) : null}
      <div className={cx('mt-2 empty:hidden', pad)}>
        <ConnectionResult profile={profile} />
      </div>
    </article>
  );
}
