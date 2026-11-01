import { useEffect, useState } from 'react';
import { Button } from '../components/Button';
import { ConfirmDialog } from '../components/Dialog';
import { Field } from '../components/Field';
import { Icons } from '../components/icons';
import { EmptyState, ScreenHeader } from '../components/Layout';
import { ProfileCard } from '../components/ProfileCard';
import { useToast } from '../components/Toast';
import { applyProfiles } from '../lib/appState';
import { sortProfiles } from '../lib/format';
import * as ipc from '../lib/ipc';
import { useNav } from '../lib/nav';
import type { IpcError, Profile, ProfileSet } from '../lib/types';
import { S } from '../strings';
import { ProfileForm } from './ProfileForm';

export function ProfilesScreen({ profiles }: { profiles: ProfileSet }) {
  const nav = useNav();
  const toast = useToast();
  const [editing, setEditing] = useState<string | 'new' | null>(null);
  const [testing, setTesting] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<Profile | null>(null);

  // Ctrl+N and "edit" requests from elsewhere.
  useEffect(() => {
    const i = nav.intent;
    if (i?.kind === 'new-profile') {
      setEditing('new');
      nav.clearIntent();
    } else if (i?.kind === 'edit-profile') {
      setEditing(i.id);
      nav.clearIntent();
    }
  }, [nav]);

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

  const test = async (p: Profile) => {
    setTesting(p.id);
    try {
      const r = await ipc.testConnection(p.id);
      if (r.kind === 'connected') toast({ message: S.profiles.connected(r.user) });
      else if (r.kind === 'denied') toast({ kind: 'warning', message: S.profiles.denied });
      else toast({ kind: 'error', message: S.profiles.failed(r.reason) });
    } catch (e) {
      toast({ kind: 'error', message: S.profiles.failed((e as IpcError).message) });
    } finally {
      setTesting(null);
    }
  };

  if (editing === 'new' || original) {
    return (
      <>
        <ProfileForm
          key={editing}
          profiles={profiles}
          original={original}
          onCancel={() => setEditing(null)}
          onSaved={(set) => {
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
            onClick={() => setEditing('new')}
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
            <Button variant="primary" icon={Icons.add} onClick={() => setEditing('new')}>
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
                testing={testing === p.id}
                onEdit={() => setEditing(p.id)}
                onMakeDefault={() => void makeDefault(p)}
                onTest={() => void test(p)}
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
        onDeleted={() => setDeleting(null)}
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
