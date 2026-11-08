import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type MutableRefObject,
  type ReactNode,
} from 'react';
import { Banner } from '../components/Banner';
import { Button } from '../components/Button';
import { ChipInput } from '../components/ChipInput';
import { ColorPicker } from '../components/ColorPicker';
import { ConnectionResult, useTestConnection } from '../components/ConnectionResult';
import { PassphraseStatus } from '../components/PassphraseStatus';
import { Checkbox, Field, PasswordInput, RequiredMark, TextInput } from '../components/Field';
import { FolderField } from '../components/FolderField';
import { Ico, Icons } from '../components/icons';
import { Avatar } from '../components/Avatar';
import { useToast } from '../components/Toast';
import { applyProfiles } from '../lib/appState';
import { useConnectionEntry } from '../lib/connection';
import { cx } from '../lib/cx';
import { NOREPLY_TEMPLATE, fileName, nextColor, normalizePath, slugify } from '../lib/format';
import * as ipc from '../lib/ipc';
import type { IpcError, Profile, ProfileColor, ProfileSet, ValidationIssue } from '../lib/types';
import { S } from '../strings';

const FIELDS = ['label', 'id', 'name', 'email', 'color', 'owners', 'folders', 'ssh_key'] as const;
type FieldName = (typeof FIELDS)[number];

/** Core's tip for a free-mail address (`profile.rs`, `is_public_mail`): a warning only. */
function isPublicEmailTip(i: ValidationIssue): boolean {
  return (
    i.field === 'email' &&
    i.severity === 'warning' &&
    i.message.startsWith('This is a public email address')
  );
}

type KeyMode = 'generate' | 'existing';

export function emptyProfile(set: ProfileSet | undefined, seed?: Partial<Profile>): Profile {
  return {
    id: '',
    label: '',
    name: '',
    email: '',
    color: nextColor(set),
    ssh_key: '',
    owners: [],
    folders: [],
    ...seed,
  };
}

export function ProfileForm({
  profiles,
  original,
  seed,
  onSaved,
  onCancel,
  onDelete,
  embedded = false,
  submitLabel,
  heading,
  autoFocusHeading = !embedded,
  focusField,
  ownersProminent = false,
  onEditProfile,
}: {
  profiles: ProfileSet;
  original?: Profile;
  /** Pre-filled values for a new profile (onboarding). */
  seed?: Partial<Profile>;
  onSaved: (set: ProfileSet, saved: Profile) => void;
  onCancel?: () => void;
  onDelete?: () => void;
  /** Inside onboarding: no sticky bar, no danger zone, no page title. */
  embedded?: boolean;
  submitLabel?: string;
  heading?: string;
  /** Move focus to the form heading on mount (screen change, added form). */
  autoFocusHeading?: boolean;
  /** Focus this field on mount instead of the heading ("Add owner" on a card). */
  focusField?: 'owners';
  /**
   * Onboarding: Owns comes right after Identity with a plain question, and an
   * empty Owns gets a warning; the first Next with no owner stops once to show it.
   */
  ownersProminent?: boolean;
  /** "Edit <other profile>" when saving needs another profile fixed first. */
  onEditProfile?: (id: string) => void;
}) {
  const toast = useToast();
  const formId = useId();
  const [draft, setDraft] = useState<Profile>(() =>
    original ? { ...original } : emptyProfile(profiles, seed),
  );
  const [idTouched, setIdTouched] = useState(!!original || !!seed?.id);
  // Opened to fix a field ("Add owner"): show its message straight away.
  const [touched, setTouched] = useState<Set<FieldName>>(
    () => new Set(focusField ? [focusField] : []),
  );
  const [submitted, setSubmitted] = useState(false);
  const [issues, setIssues] = useState<ValidationIssue[]>([]);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [showNoreplyHelp, setShowNoreplyHelp] = useState(false);

  const [keyMode, setKeyMode] = useState<KeyMode>(original?.ssh_key ? 'existing' : 'generate');
  const [suggested, setSuggested] = useState('');
  const [noPassphrase, setNoPassphrase] = useState(false);
  const [generating, setGenerating] = useState(false);
  const [publicKey, setPublicKey] = useState<string | null>(null);
  const [keyReused, setKeyReused] = useState(false);
  // Key passphrase (ADR 0007): kept only in this form's state until Generate key,
  // then cleared. Never saved.
  const [pass, setPass] = useState('');
  const [pass2, setPass2] = useState('');
  const [passTouched, setPassTouched] = useState(false);
  const passShort = !noPassphrase && pass.length > 0 && pass.length < 8;
  const passMismatch = !noPassphrase && pass2.length > 0 && pass !== pass2;
  const passReady = noPassphrase || (pass.length >= 8 && pass === pass2);
  const [keyError, setKeyError] = useState<string | null>(null);
  const [ownersNudged, setOwnersNudged] = useState(false);
  // The set as saved most recently from this form (an inline owner fix for
  // another profile), so its banner updates before the props catch up.
  const [latestSet, setLatestSet] = useState<ProfileSet | null>(null);
  const currentSet = latestSet ?? profiles;
  const others = currentSet.profile.filter((p) => p.id !== original?.id);
  // Two or more profiles once this one is saved: every profile needs an owner or folder.
  const multi = others.length >= 1;
  const othersNeedingRule = multi ? others.filter(hasNoRule) : [];
  const runTest = useTestConnection();
  const testing = useConnectionEntry(original?.id)?.status === 'testing';

  const refs = useRef<Partial<Record<FieldName, HTMLElement | null>>>({});
  const otherFixRef = useRef<HTMLElement | null>(null);
  const headingRef = useRef<HTMLHeadingElement>(null);

  // Screen readers and keyboard users start at the form, not on <body>.
  useEffect(() => {
    const target = focusField ? refs.current[focusField] : null;
    if (target) {
      target.focus();
      target.scrollIntoView?.({ block: 'center' });
    } else if (autoFocusHeading) headingRef.current?.focus();
    // eslint-disable-next-line react-hooks/exhaustive-deps -- mount only
  }, []);
  const colourLabelId = `${formId}-colour`;
  const ownsHintId = `${formId}-owns-required`;
  const colourHelpId = `${formId}-colour-help`;

  const set = <K extends keyof Profile>(k: K, v: Profile[K]) => {
    setDraft((d) => ({ ...d, [k]: v }));
    setTouched((t) => new Set(t).add(k as FieldName));
  };
  // Validation shows on blur too, not only after typing.
  const touch = (f: FieldName) => setTouched((t) => (t.has(f) ? t : new Set(t).add(f)));

  // Profile id follows the label until the user edits it.
  useEffect(() => {
    if (!idTouched) setDraft((d) => ({ ...d, id: slugify(d.label) }));
  }, [draft.label, idTouched]);

  const effectiveKey = keyMode === 'generate' ? draft.ssh_key || suggested : draft.ssh_key;

  // Live validation through the core (no write).
  useEffect(() => {
    let live = true;
    const t = window.setTimeout(() => {
      ipc
        .validateProfile({ ...draft, ssh_key: effectiveKey }, original?.id)
        .then((r) => live && setIssues(r))
        .catch(() => undefined);
    }, 250);
    return () => {
      live = false;
      window.clearTimeout(t);
    };
  }, [draft, effectiveKey, original?.id]);

  // Suggested key path for "Generate new key".
  useEffect(() => {
    if (keyMode !== 'generate' || !draft.id) {
      setSuggested('');
      return;
    }
    let live = true;
    ipc
      .suggestKeyPath(draft.id)
      .then((p) => live && setSuggested(normalizePath(p)))
      .catch(() => live && setSuggested(''));
    return () => {
      live = false;
    };
  }, [keyMode, draft.id]);

  // Existing profile: show its public key.
  useEffect(() => {
    if (!original) return;
    let live = true;
    ipc
      .readPublicKey(original.id)
      .then((k) => live && setPublicKey(k.trim()))
      .catch(() => live && setPublicKey(null));
    return () => {
      live = false;
    };
  }, [original]);

  const takenBy = useMemo(() => {
    const m: Partial<Record<ProfileColor, string>> = {};
    for (const p of profiles.profile) if (p.id !== original?.id) m[p.color] = p.label;
    return m;
  }, [profiles, original?.id]);

  // Checks only the form can make: the noreply placeholder left in, and a
  // "Generate new key" profile whose key was never created.
  const keyMissing = keyMode === 'generate' && !publicKey;
  const ownersEmpty =
    ownersProminent &&
    draft.owners.every((o) => !o.trim()) &&
    draft.folders.every((f) => !f.trim()) &&
    // The core says it itself once there are two or more profiles.
    !issues.some((i) => i.field === 'owners' && !isOther(i));
  const localIssues = useMemo(
    () =>
      localChecks(
        draft.email,
        keyMissing && !!draft.id,
        ownersEmpty && (ownersNudged || touched.has('owners')),
        ownersNudged,
      ),
    [draft.email, keyMissing, draft.id, ownersEmpty, ownersNudged, touched],
  );
  // Issues about another profile (a save refused because of it) aren't this
  // form's fields: they get their own banner with a way to fix that profile.
  const isOther = (i: ValidationIssue) =>
    !!i.profile_id && i.profile_id !== draft.id && i.profile_id !== original?.id;
  // The public-email tip is for the moment the email is chosen: once a profile
  // is saved with it, the choice is made, so it isn't repeated on every visit.
  const emailUnchanged =
    !!original && draft.email.trim().toLowerCase() === original.email.trim().toLowerCase();
  const ownIssues = issues.filter((i) => !isOther(i) && !(emailUnchanged && isPublicEmailTip(i)));
  const otherProfileIssues = issues.filter(
    (i) =>
      isOther(i) && i.severity === 'error' && !othersNeedingRule.some((o) => o.id === i.profile_id),
  );
  const allIssues = [...ownIssues, ...localIssues];

  const byField = new Map<string, ValidationIssue[]>();
  for (const i of allIssues) {
    const f = (FIELDS as readonly string[]).includes(i.field) ? i.field : '_other';
    byField.set(f, [...(byField.get(f) ?? []), i]);
  }

  const visible = (f: FieldName) =>
    submitted || touched.has(f)
      ? (byField.get(f) ?? [])
      : (byField.get(f) ?? []).filter((i) => i.severity !== 'error');
  const otherIssues = submitted ? (byField.get('_other') ?? []) : [];
  const hasErrors = allIssues.some((i) => i.severity === 'error');

  const onSubmit = async (e?: FormEvent) => {
    e?.preventDefault();
    setSubmitted(true);
    setSaveError(null);
    const toSave: Profile = { ...draft, ssh_key: effectiveKey };
    let current = issues;
    try {
      current = await ipc.validateProfile(toSave, original?.id);
      setIssues(current);
    } catch {
      // Fall through: save_profile validates again.
    }
    const firstErr = firstInvalid(
      [...current.filter((i) => !isOther(i)), ...localIssues],
      ownersProminent,
    );
    if (firstErr) {
      refs.current[firstErr]?.focus();
      return;
    }
    if (othersNeedingRule.length) {
      // Another profile has to get an owner first; its fix is at the top.
      otherFixRef.current?.focus();
      return;
    }
    // Onboarding: stop once on an empty Owns so the warning is read, not skipped past.
    if (ownersProminent && !ownersNudged && ownersEmpty) {
      setOwnersNudged(true);
      refs.current.owners?.focus();
      return;
    }
    setSaving(true);
    try {
      const result = await ipc.saveProfile(toSave, original?.id);
      toast(
        keyMissing
          ? { kind: 'warning', message: S.form.savedNoKey(toSave.label) }
          : { message: S.form.saved(toSave.label) },
      );
      onSaved(result, result.profile.find((p) => p.id === toSave.id) ?? toSave);
    } catch (err) {
      const ie = err as IpcError;
      if (ie.code === 'validation' && ie.issues.length) {
        setIssues(ie.issues);
        const f = firstInvalid(
          ie.issues.filter((i) => !isOther(i)),
          ownersProminent,
        );
        if (f) refs.current[f]?.focus();
        else otherFixRef.current?.focus();
      } else {
        setSaveError(ie.message);
      }
    } finally {
      setSaving(false);
    }
  };

  const generate = async () => {
    setKeyError(null);
    setGenerating(true);
    try {
      // Works before the profile is saved; the key path is saved with it. The
      // passphrase is used once by the backend (in memory) and then forgotten.
      const res = await ipc.generateKey(
        draft.id,
        draft.email,
        noPassphrase ? null : pass,
        noPassphrase,
      );
      setPass('');
      setPass2('');
      setPassTouched(false);
      setPublicKey(res.public_key.trim());
      setKeyReused(res.reused);
      set('ssh_key', normalizePath(res.path));
      if (res.remembered === 'remembered') {
        toast({
          message: res.reused
            ? `${S.form.keyReused(fileName(res.path))} ${S.passphrase.done}`
            : S.form.keyCreatedRemembered,
        });
      } else if (res.remembered === 'no-passphrase') {
        toast({ message: res.reused ? S.form.keyReused(fileName(res.path)) : S.form.keyCreated });
      } else {
        toast({ kind: 'warning', message: S.form.keyNotRemembered });
      }
    } catch (err) {
      setKeyError((err as IpcError).message);
    } finally {
      setGenerating(false);
    }
  };

  const loadPublicKeyFor = async (path: string) => {
    set('ssh_key', path);
    if (!original) return;
    try {
      setPublicKey((await ipc.readPublicKey(original.id)).trim());
    } catch {
      setPublicKey(null);
    }
  };

  const copyKey = async () => {
    if (!publicKey) return;
    try {
      await ipc.copyText(publicKey);
      toast({ message: S.form.keyCopied });
    } catch (err) {
      toast({ kind: 'error', message: (err as Error).message || String(err) });
    }
  };

  const copyTitle = async () => {
    try {
      await ipc.copyText(S.form.ghSteps.keyTitle(draft.label || draft.id));
      toast({ message: S.form.ghSteps.titleCopied });
    } catch (err) {
      toast({ kind: 'error', message: (err as Error).message || String(err) });
    }
  };

  const test = () => {
    if (original) void runTest(original);
  };

  // 2. Colour
  const colourSection = (
    <FormSection title={S.form.colour} titleId={colourLabelId}>
      <p id={colourHelpId} className="-mt-2 text-small text-ink-muted">
        {S.form.colourHelp}
      </p>
      <div
        ref={(el) => {
          refs.current.color = el?.querySelector<HTMLElement>('[aria-checked="true"]') ?? null;
        }}
      >
        <ColorPicker
          value={draft.color}
          onChange={(c) => set('color', c)}
          takenBy={takenBy}
          labelledBy={colourLabelId}
          describedBy={colourHelpId}
        />
      </div>
      <IssueList issues={visible('color')} />
    </FormSection>
  );

  // 3. Owns (straight after Identity in onboarding)
  const ownsSection = (
    <FormSection title={S.form.owns} required={multi} describedBy={multi ? ownsHintId : undefined}>
      {ownersProminent ? <p className="-mt-2 text-body text-ink">{S.form.ownsPrompt}</p> : null}
      {multi ? (
        <p id={ownsHintId} className={cx('text-small text-ink-muted', !ownersProminent && '-mt-2')}>
          {S.form.ownsRequired}
        </p>
      ) : null}
      <Field label={S.form.ownersLabel} helper={S.form.ownersHelp} issues={visible('owners')}>
        {(p) => (
          // Blur only observes focus leaving the chip input; the wrapper isn't interactive.
          // eslint-disable-next-line jsx-a11y/no-static-element-interactions
          <div
            onBlur={() => touch('owners')}
            ref={(el) => {
              refs.current.owners = el?.querySelector('input') ?? null;
            }}
          >
            <ChipInput
              control={p}
              value={draft.owners}
              onChange={(v) => set('owners', v)}
              placeholder={S.form.ownersPlaceholder}
            />
          </div>
        )}
      </Field>
      <Field
        label={S.form.foldersLabel}
        helper={S.form.foldersHelp}
        issues={visible('folders')}
        labelAs="div"
      >
        {() => (
          <div className="flex flex-col gap-2">
            {draft.folders.map((f, i) => (
              <FolderField
                key={`${f}-${i}`}
                value={f}
                control={{ id: `${formId}-folder-${i}`, 'aria-describedby': undefined }}
                pickTitle={S.form.chooseFolder}
                onChange={(v) =>
                  set(
                    'folders',
                    draft.folders.map((x, j) => (j === i ? v : x)),
                  )
                }
                onRemove={() =>
                  set(
                    'folders',
                    draft.folders.filter((_, j) => j !== i),
                  )
                }
                removeLabel={S.common.remove(f)}
              />
            ))}
            <div>
              <Button
                icon={Icons.add}
                ref={(el) => {
                  refs.current.folders = el;
                }}
                onClick={async () => {
                  const p = await ipc.pickFolder(S.form.chooseFolder);
                  if (p && !draft.folders.includes(normalizePath(p)))
                    set('folders', [...draft.folders, normalizePath(p)]);
                }}
              >
                {S.form.addFolder}
              </Button>
            </div>
          </div>
        )}
      </Field>
    </FormSection>
  );

  const title = heading ?? (original ? S.form.editTitle(original.label) : S.form.addTitle);

  return (
    <form
      onSubmit={onSubmit}
      noValidate
      aria-labelledby={`${formId}-title`}
      className="flex flex-col"
    >
      <div className={cx('flex items-center gap-3', embedded ? 'mb-4' : 'mb-6')}>
        <Avatar label={draft.label || '?'} color={draft.color} size={40} decorative />
        {embedded ? (
          <h2 id={`${formId}-title`} ref={headingRef} tabIndex={-1} className="text-heading">
            {title}
          </h2>
        ) : (
          <h1 id={`${formId}-title`} ref={headingRef} tabIndex={-1} className="text-title">
            {title}
          </h1>
        )}
      </div>

      {/* Explains the * after required labels; screen readers hear aria-required. */}
      <p aria-hidden="true" className="-mt-2 mb-4 text-small text-ink-muted">
        <span className="mr-0.5">*</span>
        {S.form.requiredLegend}
      </p>

      <div className="flex flex-col gap-8">
        {othersNeedingRule.length ? (
          <OtherProfilesFix
            profiles={othersNeedingRule}
            focusRef={otherFixRef}
            onEditProfile={onEditProfile}
            onSaved={(next) => {
              setLatestSet(next);
              applyProfiles(next);
              setIssues((list) => list.filter((i) => !isOther(i)));
            }}
          />
        ) : null}
        {otherProfileIssues.length ? (
          <Banner kind="blocked">
            <p>{S.form.otherIssuesTitle}</p>
            <ul className="mt-1 flex flex-col gap-1">
              {otherProfileIssues.map((i, n) => {
                const who = currentSet.profile.find((p) => p.id === i.profile_id);
                return (
                  <li key={n} className="flex flex-wrap items-center gap-2">
                    <span className="min-w-0 flex-1">{i.message}</span>
                    {who && onEditProfile ? (
                      <Button compact icon={Icons.edit} onClick={() => onEditProfile(who.id)}>
                        {S.form.editOther(who.label)}
                      </Button>
                    ) : null}
                  </li>
                );
              })}
            </ul>
          </Banner>
        ) : null}
        {otherIssues.length ? (
          <Banner kind="blocked">
            <ul>
              {otherIssues.map((i, n) => (
                <li key={n}>{i.message}</li>
              ))}
            </ul>
          </Banner>
        ) : null}

        {/* 1. Identity */}
        <FormSection title={S.form.identity}>
          <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
            <Field
              label={S.form.label}
              helper={S.form.labelHelp}
              issues={visible('label')}
              required
            >
              {(p) => (
                <TextInput
                  {...p}
                  onBlur={() => touch('label')}
                  ref={(el) => {
                    refs.current.label = el;
                  }}
                  value={draft.label}
                  onChange={(e) => set('label', e.target.value)}
                  maxLength={40}
                />
              )}
            </Field>
            <Field label={S.form.id} helper={S.form.idHelp(draft.id)} issues={visible('id')}>
              {(p) => (
                <TextInput
                  {...p}
                  ref={(el) => {
                    refs.current.id = el;
                  }}
                  mono
                  value={draft.id}
                  onChange={(e) => {
                    setIdTouched(true);
                    set('id', e.target.value.toLowerCase());
                  }}
                  maxLength={32}
                />
              )}
            </Field>
          </div>
          <Field label={S.form.name} helper={S.form.nameHelp} issues={visible('name')} required>
            {(p) => (
              <TextInput
                {...p}
                onBlur={() => touch('name')}
                ref={(el) => {
                  refs.current.name = el;
                }}
                value={draft.name}
                onChange={(e) => set('name', e.target.value)}
                autoComplete="name"
              />
            )}
          </Field>
          <Field
            label={S.form.email}
            helper={
              <span className="flex flex-col gap-1">
                <span>{S.form.emailHelp}</span>
                {showNoreplyHelp ? <span>{S.form.noreplyHelp}</span> : null}
              </span>
            }
            issues={visible('email')}
            required
          >
            {(p) => (
              <div className="flex items-center gap-2">
                <TextInput
                  {...p}
                  onBlur={() => touch('email')}
                  ref={(el) => {
                    refs.current.email = el;
                  }}
                  mono
                  type="email"
                  value={draft.email}
                  onChange={(e) => set('email', e.target.value)}
                />
                <Button
                  onClick={() => {
                    set('email', NOREPLY_TEMPLATE(draft.owners[0] ?? ''));
                    setShowNoreplyHelp(true);
                    refs.current.email?.focus();
                  }}
                >
                  {S.form.useNoreply}
                </Button>
              </div>
            )}
          </Field>
        </FormSection>

        {ownersProminent ? (
          <>
            {ownsSection}
            {colourSection}
          </>
        ) : (
          <>
            {colourSection}
            {ownsSection}
          </>
        )}

        {/* 4. SSH key */}
        <FormSection title={S.form.sshKey} required>
          <fieldset className="flex flex-wrap gap-x-6 gap-y-2">
            <legend className="sr-only">{S.form.keyModeLabel}</legend>
            {(['generate', 'existing'] as const).map((m) => (
              <label key={m} className="flex items-center gap-2 text-body">
                <input
                  type="radio"
                  name={`${formId}-keymode`}
                  checked={keyMode === m}
                  onChange={() => setKeyMode(m)}
                />
                {m === 'generate' ? S.form.keyGenerate : S.form.keyExisting}
              </label>
            ))}
          </fieldset>

          {keyMode === 'generate' ? (
            <div className="flex flex-col gap-3 rounded-lg border border-line bg-surface p-4">
              {suggested ? (
                <p className="font-mono text-mono text-ink-muted">{S.form.willCreate(suggested)}</p>
              ) : null}
              {publicKey ? null : (
                <>
                  <Banner kind="info" live={false}>
                    {S.form.passphraseNote}
                  </Banner>
                  {noPassphrase ? null : (
                    <div className="grid gap-3 sm:grid-cols-2">
                      <Field
                        label={S.form.passphrase}
                        helper={S.form.passphraseHelp}
                        issues={
                          passTouched && passShort
                            ? [{ severity: 'error', message: S.form.passphraseShort }]
                            : []
                        }
                      >
                        {(p) => (
                          <PasswordInput
                            {...p}
                            value={pass}
                            onChange={(e) => setPass(e.target.value)}
                            onBlur={() => setPassTouched(true)}
                          />
                        )}
                      </Field>
                      <Field
                        label={S.form.passphraseConfirm}
                        issues={
                          passMismatch
                            ? [{ severity: 'error', message: S.form.passphraseMismatch }]
                            : []
                        }
                      >
                        {(p) => (
                          <PasswordInput
                            {...p}
                            value={pass2}
                            onChange={(e) => setPass2(e.target.value)}
                            onKeyDown={(e) => {
                              if (e.key === 'Enter' && passReady && draft.id) {
                                e.preventDefault();
                                void generate();
                              }
                            }}
                          />
                        )}
                      </Field>
                    </div>
                  )}
                  <Checkbox
                    label={S.form.noPassphrase}
                    checked={noPassphrase}
                    onChange={setNoPassphrase}
                  />
                </>
              )}
              <div className={publicKey ? 'hidden' : undefined}>
                <Button
                  icon={Icons.key}
                  onClick={generate}
                  loading={generating}
                  disabled={!draft.id || !passReady}
                  ref={(el) => {
                    if (keyMode === 'generate') refs.current.ssh_key = el;
                  }}
                >
                  {generating ? S.form.generating : S.form.generate}
                </Button>
              </div>
              {keyError ? <Banner kind="blocked">{keyError}</Banner> : null}
              <IssueList issues={visible('ssh_key')} />
            </div>
          ) : (
            <Field
              label={S.form.keyPath}
              helper={S.form.keyPathHelp}
              issues={visible('ssh_key')}
              required
            >
              {(p) => (
                <div
                  ref={(el) => {
                    if (keyMode === 'existing')
                      refs.current.ssh_key = el?.querySelector('input') ?? null;
                  }}
                >
                  <FolderField
                    mode="file"
                    value={draft.ssh_key}
                    control={p}
                    pickTitle={S.form.chooseKey}
                    emptyText={S.form.noKeyChosen}
                    onChange={loadPublicKeyFor}
                  />
                </div>
              )}
            </Field>
          )}

          {publicKey ? (
            <div className="flex flex-col gap-2">
              {/* Guided: how to add this key on GitHub, step by step, in the app. */}
              <section
                aria-labelledby={`${formId}-gh-steps`}
                className="flex flex-col gap-3 rounded-lg border border-line bg-surface px-4 py-3"
              >
                <h3 id={`${formId}-gh-steps`} className="text-body font-medium text-ink">
                  {S.form.ghSteps.title}
                </h3>
                <p className="-mt-2 text-small text-ink-muted">
                  {keyReused ? S.form.ghSteps.introReused : S.form.ghSteps.intro}
                </p>
                <ol className="flex list-decimal flex-col gap-4 pl-5 marker:text-ink-muted">
                  <li className="flex flex-col gap-2">
                    <span>{S.form.ghSteps.copy}</span>
                    <Field label={S.form.publicKey}>
                      {(p) => (
                        <textarea
                          {...p}
                          readOnly
                          value={publicKey}
                          rows={3}
                          className="gh-input w-full resize-none break-all rounded-md border border-line-input bg-bg px-2.5 py-1.5 font-mono text-mono-small text-ink"
                        />
                      )}
                    </Field>
                    <div>
                      <Button icon={Icons.copy} onClick={copyKey}>
                        {S.form.copyKey}
                      </Button>
                    </div>
                  </li>
                  <li className="flex flex-col gap-2">
                    <span>{S.form.ghSteps.open(draft.owners.filter((o) => o.trim()))}</span>
                    <div>
                      <Button
                        icon={Icons.external}
                        onClick={() => void ipc.openExternal('ssh-keys')}
                      >
                        {S.form.openGithub}
                      </Button>
                    </div>
                  </li>
                  <li className="flex flex-col gap-2">
                    <span>{S.form.ghSteps.fill}</span>
                    <dl className="grid grid-cols-[auto_1fr] items-baseline gap-x-4 gap-y-1 text-small">
                      <dt className="text-ink-muted">{S.form.ghSteps.titleLabel}</dt>
                      <dd className="flex flex-wrap items-center gap-2">
                        <span className="font-mono text-mono-small text-ink">
                          {S.form.ghSteps.keyTitle(draft.label || draft.id)}
                        </span>
                        <Button compact icon={Icons.copy} onClick={() => void copyTitle()}>
                          {S.form.ghSteps.copyTitle}
                        </Button>
                      </dd>
                      <dt className="text-ink-muted">{S.form.ghSteps.typeLabel}</dt>
                      <dd className="text-ink">{S.form.ghSteps.typeValue}</dd>
                      <dt className="text-ink-muted">{S.form.ghSteps.keyLabel}</dt>
                      <dd className="text-ink">{S.form.ghSteps.keyValue}</dd>
                    </dl>
                    <span>{S.form.ghSteps.add}</span>
                  </li>
                  {draft.sign_commits ? (
                    // ADR 0008: GitHub needs the same key a second time, as a Signing Key.
                    <li className="flex flex-col gap-2">
                      <span>{S.form.ghSteps.signing}</span>
                      <dl className="grid grid-cols-[auto_1fr] items-baseline gap-x-4 gap-y-1 text-small">
                        <dt className="text-ink-muted">{S.form.ghSteps.titleLabel}</dt>
                        <dd className="font-mono text-mono-small text-ink">
                          {S.form.ghSteps.signingTitle(draft.label || draft.id)}
                        </dd>
                        <dt className="text-ink-muted">{S.form.ghSteps.typeLabel}</dt>
                        <dd className="text-ink">{S.form.ghSteps.signingType}</dd>
                      </dl>
                    </li>
                  ) : null}
                  <li className="flex flex-col gap-2">
                    <span>{original ? S.form.ghSteps.test : S.form.ghSteps.testAfterSave}</span>
                    {original ? (
                      <div>
                        <Button icon={Icons.test} onClick={test} loading={testing}>
                          {testing ? S.form.testing : S.form.testConnection}
                        </Button>
                      </div>
                    ) : null}
                  </li>
                </ol>
              </section>
              {original ? (
                <ConnectionResult
                  profile={{ id: original.id, owners: draft.owners }}
                  publicKey={publicKey}
                  showTesting={false}
                  actions={false}
                />
              ) : null}
              {original?.ssh_key ? (
                <PassphraseStatus profileId={original.id} keyPath={original.ssh_key} />
              ) : null}
            </div>
          ) : original && keyMode === 'existing' && draft.ssh_key ? (
            <p className="text-small text-ink-muted">{S.form.noPublicKey}</p>
          ) : null}

          {/* Commit signing (ADR 0008): same key, GitHub shows commits as Verified. */}
          <div className="flex flex-col gap-1 border-t border-line pt-4">
            <Checkbox
              label={S.form.signCommits}
              checked={!!draft.sign_commits}
              onChange={(v) => set('sign_commits', v)}
            />
            <p className="pl-6 text-small text-ink-muted">{S.form.signCommitsHelp}</p>
          </div>
        </FormSection>

        {/* 5. Danger zone */}
        {original && onDelete && !embedded ? (
          <FormSection title={S.form.dangerZone}>
            <div className="flex flex-wrap items-center gap-4 rounded-lg border border-blocked-border bg-surface px-4 py-3">
              <p className="min-w-[12rem] flex-1 text-small text-ink-muted">{S.form.deleteHelp}</p>
              <Button variant="danger" icon={Icons.delete} onClick={onDelete}>
                {S.form.deleteProfile}
              </Button>
            </div>
          </FormSection>
        ) : null}
      </div>

      <div
        className={cx(
          'flex flex-wrap items-center justify-end gap-2 py-3',
          embedded ? 'mt-6' : 'sticky bottom-0 z-10 -mx-6 mt-8 border-t border-line bg-bg px-6',
        )}
      >
        {submitted && hasErrors ? (
          <p className="mr-auto flex items-center gap-1 text-small text-blocked-text" role="alert">
            <Ico icon={Icons.warning} />
            {S.form.fixErrors}
          </p>
        ) : null}
        {saveError ? (
          <p className="mr-auto flex items-center gap-1 text-small text-blocked-text" role="alert">
            <Ico icon={Icons.warning} />
            {saveError}
          </p>
        ) : null}
        {onCancel ? <Button onClick={onCancel}>{S.common.cancel}</Button> : null}
        <Button variant="primary" type="submit" loading={saving}>
          {saving ? S.form.saving : (submitLabel ?? S.form.save)}
        </Button>
      </div>
    </form>
  );
}

function FormSection({
  title,
  titleId,
  required = false,
  describedBy,
  children,
}: {
  title: string;
  titleId?: string;
  /** `*` after the heading (the group is required as a whole). */
  required?: boolean;
  describedBy?: string;
  children: ReactNode;
}) {
  const autoId = useId();
  const id = titleId ?? autoId;
  return (
    <section aria-labelledby={id} aria-describedby={describedBy} className="flex flex-col gap-4">
      <h2 id={id} className="text-heading">
        {title}
        {required ? <RequiredMark /> : null}
      </h2>
      {children}
    </section>
  );
}

const hasNoRule = (p: Profile) =>
  p.owners.every((o) => !o.trim()) && p.folders.every((f) => !f.trim());

/** First field with an error, in the order the form shows them. */
function firstInvalid(issues: ValidationIssue[], ownsFirst: boolean): FieldName | undefined {
  const order: FieldName[] = ownsFirst
    ? ['label', 'id', 'name', 'email', 'owners', 'folders', 'color', 'ssh_key']
    : ['label', 'id', 'name', 'email', 'color', 'owners', 'folders', 'ssh_key'];
  const bad = new Set(issues.filter((i) => i.severity === 'error').map((i) => i.field));
  return order.find((f) => bad.has(f));
}

/**
 * Saving a second profile needs every profile to have an owner or folder. When
 * another one has neither (e.g. Office from the first onboarding step), fix it
 * here: one owner field per profile, saved on its own, so nobody dead-ends.
 */
function OtherProfilesFix({
  profiles,
  focusRef,
  onEditProfile,
  onSaved,
}: {
  profiles: Profile[];
  focusRef: MutableRefObject<HTMLElement | null>;
  onEditProfile?: (id: string) => void;
  onSaved: (set: ProfileSet) => void;
}) {
  return (
    <Banner kind="warning">
      <div className="flex flex-col gap-3">
        {profiles.map((p, n) => (
          <OtherProfileOwner
            key={p.id}
            profile={p}
            inputRef={n === 0 ? focusRef : undefined}
            onEditProfile={onEditProfile}
            onSaved={onSaved}
          />
        ))}
      </div>
    </Banner>
  );
}

function OtherProfileOwner({
  profile,
  inputRef,
  onEditProfile,
  onSaved,
}: {
  profile: Profile;
  inputRef?: MutableRefObject<HTMLElement | null>;
  onEditProfile?: (id: string) => void;
  onSaved: (set: ProfileSet) => void;
}) {
  const toast = useToast();
  const [owner, setOwner] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const add = async () => {
    const o = owner.trim().toLowerCase();
    if (!o) {
      setError(S.form.otherOwnerEmpty);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const fresh = await ipc.listProfiles();
      const p = fresh.profile.find((x) => x.id === profile.id) ?? profile;
      const set = await ipc.saveProfile({ ...p, owners: [...p.owners, o] }, p.id);
      toast({ message: S.form.otherOwnerAdded(o, p.label) });
      onSaved(set);
    } catch (e) {
      const ie = e as IpcError;
      const own = ie.issues?.filter((i) => i.severity === 'error').map((i) => i.message) ?? [];
      setError(own.length ? own.join(' ') : ie.message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="flex flex-col gap-2">
      <p>
        <span className="font-medium">{S.form.otherNeedsOwner(profile.label)}</span>{' '}
        {S.form.otherNeedsOwnerWhy}
      </p>
      <Field
        label={S.form.otherOwnerLabel(profile.label)}
        issues={error ? [{ severity: 'error', message: error }] : []}
        required
      >
        {(c) => (
          <div className="flex flex-wrap items-center gap-2">
            <TextInput
              {...c}
              ref={(el) => {
                if (inputRef) inputRef.current = el;
              }}
              mono
              className="min-w-[12rem] flex-1"
              value={owner}
              placeholder={S.form.ownersPlaceholderWork}
              onChange={(e) => setOwner(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  e.preventDefault();
                  void add();
                }
              }}
            />
            <Button icon={Icons.add} loading={busy} onClick={() => void add()}>
              {S.form.addToOther(profile.label)}
            </Button>
            {onEditProfile ? (
              <Button variant="ghost" icon={Icons.edit} onClick={() => onEditProfile(profile.id)}>
                {S.form.editOther(profile.label)}
              </Button>
            ) : null}
          </div>
        )}
      </Field>
    </div>
  );
}

/** The "Use noreply" template still has its `ID+` placeholder. */
const NOREPLY_PLACEHOLDER = /^ID\+[^@]*@users\.noreply\.github\.com$/;

function localChecks(
  email: string,
  keyMissing: boolean,
  ownersEmpty: boolean,
  nudged: boolean,
): ValidationIssue[] {
  const out: ValidationIssue[] = [];
  if (ownersEmpty)
    out.push({
      field: 'owners',
      severity: 'warning',
      message: nudged ? `${S.form.ownersEmpty} ${S.form.ownersSkip}` : S.form.ownersEmpty,
    });
  if (NOREPLY_PLACEHOLDER.test(email.trim()))
    out.push({ field: 'email', severity: 'error', message: S.form.noreplyPlaceholder });
  if (keyMissing)
    out.push({ field: 'ssh_key', severity: 'warning', message: S.form.keyNotCreated });
  return out;
}

function IssueList({ issues }: { issues: ValidationIssue[] }) {
  if (!issues.length) return null;
  return (
    <ul className="flex flex-col gap-0.5">
      {issues.map((i, n) => (
        <li
          key={n}
          className={cx(
            'flex items-start gap-1 text-small',
            i.severity === 'error'
              ? 'text-blocked-text'
              : i.severity === 'warning'
                ? 'text-warning-ink'
                : 'text-ink-muted',
          )}
        >
          <Ico icon={i.severity === 'info' ? Icons.info : Icons.warning} className="mt-px" />
          {i.message}
        </li>
      ))}
    </ul>
  );
}
