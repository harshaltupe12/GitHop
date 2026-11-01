import { useEffect, useState } from 'react';
import { Button } from '../components/Button';
import { useTestConnection } from '../components/ConnectionResult';
import { ConfirmDialog } from '../components/Dialog';
import { Field } from '../components/Field';
import { Icons } from '../components/icons';
import { EmptyState, ScreenHeader } from '../components/Layout';
import { ProfileCard } from '../components/ProfileCard';
import { useToast } from '../components/Toast';
import { applyProfiles } from '../lib/appState';
import { forgetConnectionTest } from '../lib/connection';
import { sortProfiles } from '../lib/format';
import * as ipc from '../lib/ipc';
import { useNav } from '../lib/nav';
import type { IpcError, Profile, ProfileSet, ValidationIssue } from '../lib/types';
import { S } from '../strings';
import { ProfileForm } from './ProfileForm';

export function ProfilesScreen({ profiles }: { profiles: ProfileSet }) {
  const nav = useNav();
  const toast = useToast();
  const [editing, setEditing] = useState<string | 'new' | null>(null);
  // "Add owner" opens Edit with the Owns field focused.
  const [focusOwners, setFocusOwners] = useState(false);
  const [deleting, setDeleting] = useState<Profile | null>(null);
  const [issues, setIssues] = useState<Record<string, ValidationIssue[]>>({});
  const runTest = useTestConnection();

  // Ctrl+N and "edit" requests from elsewhere.
  useEffect(() => {
    const i = nav.intent;
    if (i?.kind === 'new-profile') {
      setEditing('new');
      setFocusOwners(false);
      nav.clearIntent();
    } else if (i?.kind === 'edit-profile') {
      setEditing(i.id);
      setFocusOwners(i.focus === 'owners');
      nav.clearIntent();
    }
  }, [nav]);

  // The core's warnings for each saved profile (e.g. no owners: its repos would
  // quietly use the default profile). Read-only validation, no write.
  useEffect(() => {
    let live = true;
    void Promise.all(
      profiles.profile.map((p) =>
        ipc
          .validateProfile(p, p.id)
          .then((r) => [p.id, r] as const)
          .catch(() => [p.id, [] as ValidationIssue[]] as const),
      ),
    ).then((entries) => {
      if (live) setIssues(Object.fromEntries(entries));
    });
    return () => {
      live = false;
    };
  }, [profiles]);

  const edit = (id: string | 'new', owners = false) => {
    setFocusOwners(owners);
    setEditing(id);
  };

  const list = sortProfiles(profiles);
  const original =
    editing && editing !== 'new' ? profiles.profile.find((p) => p.id === editing) : undefined;

  const makeDefault = async (p: Profile) => {
    try {
      applyProfiles(await ipc.setDefault(p.id));
      toast({ message: S.profiles.defaultSet(p.label) });
    } catch (e) {
      toast({ kind: 'error', message: (e as IpcError).message });
    }
  };


  if (editing === 'new' || original) {
    return (
      <>
        <ProfileForm
          key={editing}
          profiles={profiles}
          original={original}
          focusField={focusOwners ? 'owners' : undefined}
          onCancel={() => setEditing(null)}
          onSaved={(set, saved) => {
            // A new key or id makes the last test result meaningless.
            if (original && (original.id !== saved.id || original.ssh_key !== saved.ssh_key))
              forgetConnectionTest(original.id);
            applyProfiles(set);
            setEditing(null);
          }}
          onDelete={original ? () => setDeleting(original) : undefined}
        />
        <DeleteProfileDialog
          profile={deleting}
          profiles={profiles}
          onClose={() => setDeleting(null)}
          onDeleted={() => {
            setDeleting(null);
            setEditing(null);
          }}
        />
      </>
    );
  }

  return (
    <div>
      <ScreenHeader
        title={S.profiles.title}
        actions={
          <Button
            variant="primary"
            icon={Icons.add}
            onClick={() => edit('new')}
            aria-keyshortcuts="Control+N"
          >
            {S.profiles.add}
          </Button>
        }
      />
      {list.length === 0 ? (
        <EmptyState
          icon={Icons.emptyProfiles}
          title={S.profiles.emptyTitle}
          body={S.profiles.emptyBody}
          action={
            <Button variant="primary" icon={Icons.add} onClick={() => edit('new')}>
              {S.profiles.add}
            </Button>
          }
        />
      ) : (
        <ul className="flex flex-col gap-3" aria-label={S.profiles.listLabel}>
          {list.map((p) => (
            <li key={p.id}>
              <ProfileCard
                profile={p}
                isDefault={p.id === profiles.default}
                issues={issues[p.id]}
                onEdit={() => edit(p.id)}
                onAddOwner={() => edit(p.id, true)}
                onMakeDefault={() => void makeDefault(p)}
                onTest={() => void runTest(p)}
                onDelete={() => setDeleting(p)}
              />
            </li>
          ))}
        </ul>
      )}
      <DeleteProfileDialog
        profile={deleting}
        profiles={profiles}
        onClose={() => setDeleting(null)}
        onDeleted={() => {
          if (deleting) forgetConnectionTest(deleting.id);
          setDeleting(null);
        }}
      />
    </div>
  );
}

/** Typed confirmation (Design §7.13): type the profile label to enable Delete. */
export function DeleteProfileDialog({
  profile,
  profiles,
  onClose,
  onDeleted,
}: {
  profile: Profile | null;
  profiles: ProfileSet;
  onClose: () => void;
  onDeleted: () => void;
}) {
  const toast = useToast();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const others = profiles.profile.filter((p) => p.id !== profile?.id);
  const needsNewDefault = !!profile && profile.id === profiles.default && others.length > 0;
  const [newDefault, setNewDefault] = useState('');

  useEffect(() => {
    setError(null);
    setNewDefault(others[0]?.id ?? '');
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [profile?.id]);

  const confirm = async () => {
    if (!profile) return;
    setBusy(true);
    setError(null);
    try {
      const set = await ipc.deleteProfile(profile.id, needsNewDefault ? newDefault : undefined);
      applyProfiles(set);
      toast({ message: S.deleteDialog.deleted(profile.label) });
      onDeleted();
    } catch (e) {
      setError((e as IpcError).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <ConfirmDialog
      open={!!profile}
      title={profile ? S.deleteDialog.title(profile.label) : ''}
      word={profile?.label}
      typeLabel={profile ? S.deleteDialog.typeToConfirm(profile.label) : undefined}
      confirmLabel={S.deleteDialog.confirm}
      onConfirm={confirm}
      onClose={onClose}
      busy={busy}
      extra={
        <>
          {needsNewDefault ? (
            <Field label={S.deleteDialog.newDefault}>
              {(p) => (
                <select
                  {...p}
                  value={newDefault}
                  onChange={(e) => setNewDefault(e.target.value)}
                  className="gh-input min-h-8 w-full rounded-md border border-line-input bg-surface px-2 text-body"
                >
                  {others.map((o) => (
                    <option key={o.id} value={o.id}>
                      {o.label}
                    </option>
                  ))}
                </select>
              )}
            </Field>
          ) : null}
          {error ? (
            <p role="alert" className="text-small text-blocked-text">
              {error}
            </p>
          ) : null}
        </>
      }
    >
      {S.deleteDialog.body}
    </ConfirmDialog>
  );
}
