import { useCallback, useEffect, useState, type ReactNode } from 'react';
import { Avatar } from '../components/Avatar';
import { Banner } from '../components/Banner';
import { Button } from '../components/Button';
import { ErrorText } from '../components/ErrorText';
import { Ico, Icons } from '../components/icons';
import { EmptyState, ScreenHeader } from '../components/Layout';
import { Pill } from '../components/Pill';
import { Spinner } from '../components/Spinner';
import { Table, type Column } from '../components/Table';
import { useToast } from '../components/Toast';
import { reposStore, setUnpushedOffenders } from '../lib/appState';
import { baseName, normalizePath, profileByEmail, profileById, shortSha } from '../lib/format';
import * as ipc from '../lib/ipc';
import { useNav } from '../lib/nav';
import type { FixPlan, FixResult, IpcError, OffendingCommit, ProfileSet } from '../lib/types';
import { S } from '../strings';

const wrongEmail = (c: OffendingCommit) => (c.author_mismatch ? c.author_email : c.committer_email);

export function FixScreen({ profiles }: { profiles: ProfileSet }) {
  const nav = useNav();
  const toast = useToast();
  const repos = reposStore.useStore();
  const [repo, setRepo] = useState<string | null>(null);
  const [plan, setPlan] = useState<FixPlan | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<IpcError | null>(null);
  const [fixing, setFixing] = useState(false);
  const [result, setResult] = useState<{ res: FixResult; count: number } | null>(null);
  const [undoing, setUndoing] = useState(false);

  useEffect(() => {
    if (nav.intent?.kind === 'fix-repo') {
      setRepo(nav.intent.repo);
      setResult(null);
      nav.clearIntent();
    }
  }, [nav]);

  const load = useCallback(async (r: string) => {
    setLoading(true);
    setError(null);
    try {
      const p = await ipc.fixPlan(r);
      setPlan(p);
      // Same rule as the backend's RepoRow.unpushed_offenders: only what Fix can change.
      setUnpushedOffenders(r, p.blocked_reason || !p.profile_id ? 0 : p.offenders.length);
    } catch (e) {
      setPlan(null);
      setError(e as IpcError);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (repo) void load(repo);
    else setPlan(null);
  }, [repo, load]);

  const chooseRepo = async () => {
    const p = await ipc.pickFolder(S.fix.chooseRepoTitle).catch(() => null);
    if (p) {
      setResult(null);
      setRepo(normalizePath(p));
    }
  };

  const undo = async (r: string, backupRef: string) => {
    setUndoing(true);
    try {
      await ipc.fixUndo(r, backupRef);
      setResult(null);
      toast({ kind: 'info', message: S.fix.undone });
      await load(r);
    } catch (e) {
      toast({ kind: 'error', message: (e as IpcError).message });
    } finally {
      setUndoing(false);
    }
  };

  const apply = async () => {
    if (!repo || !plan) return;
    setFixing(true);
    try {
      const res = await ipc.fixApply(repo);
      const count = res.rewritten || plan.offenders.length;
      setResult({ res, count });
      toast({
        message: S.fix.toastDone(count),
        action: { label: S.common.undo, onClick: () => void undo(repo, res.backup_ref) },
      });
      await load(repo);
    } catch (e) {
      toast({ kind: 'error', message: (e as IpcError).message });
    } finally {
      setFixing(false);
    }
  };

  const openDocs = async () => {
    try {
      await ipc.openExternal('docs');
    } catch (e) {
      toast({ kind: 'error', message: (e as IpcError).message });
    }
  };

  const targetProfile = profileById(profiles, plan?.profile_id);
  const header = (
    <ScreenHeader
      title={S.fix.title}
      aside={
        repo ? (
          <span title={repo}>
            {baseName(repo)}
            {targetProfile ? ` · ${targetProfile.label}` : ''}
          </span>
        ) : undefined
      }
      actions={
        <Button icon={Icons.folder} onClick={() => void chooseRepo()}>
          {repo ? S.fix.otherRepo : S.fix.chooseRepo}
        </Button>
      }
    />
  );

  // ---- No repo selected: picker or "nothing to fix" ---------------------------
  if (!repo) {
    const withOffenders = repos.repos.filter((r) => r.unpushed_offenders > 0);
    return (
      <div>
        {header}
        {withOffenders.length ? (
          <section aria-labelledby="fix-pick" className="flex flex-col gap-3">
            <div>
              <h2 id="fix-pick" className="text-heading">
                {S.fix.pickTitle}
              </h2>
              <p className="text-small text-ink-muted">{S.fix.pickBody}</p>
            </div>
            <ul className="flex flex-col gap-2">
              {withOffenders.map((r) => {
                const p = profileById(profiles, r.resolution.profile_id);
                const n = r.unpushed_offenders;
                return (
                  <li key={r.root}>
                    <button
                      type="button"
                      onClick={() => {
                        setResult(null);
                        setRepo(r.root);
                      }}
                      className="gh-card transition-ui flex w-full flex-wrap items-center gap-3 rounded-lg border border-line bg-surface px-4 py-3 text-left hover:border-line-input hover:bg-surface-hover"
                    >
                      <span className="min-w-0 flex-1 truncate font-mono text-mono" title={r.root}>
                        {baseName(r.root)}
                      </span>
                      {p ? (
                        <span className="flex items-center gap-2 text-body">
                          <Avatar label={p.label} color={p.color} size={22} decorative />
                          {p.label}
                        </span>
                      ) : null}
                      <Pill kind="blocked">{S.pill.wrongCommits(n)}</Pill>
                    </button>
                  </li>
                );
              })}
            </ul>
          </section>
        ) : (
          <EmptyState icon={Icons.emptyOk} iconClassName="text-aqua-strong" body={S.fix.nothing} />
        )}
      </div>
    );
  }

  // ---- Repo selected ------------------------------------------------------------
  const columns: Column<OffendingCommit>[] = [
    {
      key: 'sha',
      header: S.fix.colCommit,
      render: (c) => (
        <span className="tabular font-mono text-mono-small" title={c.sha}>
          {shortSha(c.sha)}
        </span>
      ),
    },
    {
      key: 'msg',
      header: S.fix.colMessage,
      className: 'max-w-[18rem]',
      render: (c) => (
        <span className="block truncate" title={c.subject}>
          {c.subject}
        </span>
      ),
    },
    {
      key: 'email',
      header: S.fix.colEmail,
      render: (c) => (
        <span className="font-mono text-mono-small text-blocked-text" title={wrongEmail(c)}>
          {wrongEmail(c)}
        </span>
      ),
    },
  ];

  let body: ReactNode = null;
  if (loading && !plan) {
    body = (
      <p className="flex items-center gap-2 text-body text-ink-muted" role="status">
        <Spinner />
        {S.fix.loadingPlan}
      </p>
    );
  } else if (error) {
    body = (
      <Banner kind="blocked" action={{ label: S.common.retry, onClick: () => void load(repo) }}>
        <ErrorText error={error} />
      </Banner>
    );
  } else if (plan) {
    const unpushed = plan.offenders;
    const pushed = plan.pushed_offenders;
    const all = [...unpushed, ...pushed];
    const first = all[0];
    // Both kinds: two labelled tables so it's clear which rows Fix changes.
    const split = unpushed.length > 0 && pushed.length > 0;
    const canFix = unpushed.length > 0 && !plan.blocked_reason && !!plan.profile_id;
    const wrongProfile = first ? profileByEmail(profiles, wrongEmail(first)) : undefined;
    const repoLabel = targetProfile?.label ?? '';
    body = (
      <div className="flex flex-col gap-4">
        {result ? (
          <Banner
            kind="success"
            action={{
              label: S.common.undo,
              onClick: () => void undo(repo, result.res.backup_ref),
              busy: undoing,
            }}
          >
            {result.res.fast_forward === false
              ? S.fix.doneDiverged(result.count)
              : S.fix.done(result.count)}
            <span className="mt-1 flex items-center gap-1 text-small">
              <Ico icon={Icons.shield} />
              <span className="font-mono text-mono-small">
                {S.fix.backup(result.res.backup_ref)}
              </span>
            </span>
          </Banner>
        ) : null}

        {all.length === 0 ? (
          result ? null : (
            <EmptyState
              icon={Icons.emptyOk}
              iconClassName="text-aqua-strong"
              body={S.fix.nothing}
            />
          )
        ) : (
          <>
            {plan.blocked_reason ? (
              <Banner kind="warning">{S.fix.cantFix(plan.blocked_reason)}</Banner>
            ) : !plan.profile_id ? (
              <Banner kind="warning">{S.fix.unresolved}</Banner>
            ) : unpushed.length ? (
              <Banner kind="blocked">
                {S.fix.blocked(
                  unpushed.length,
                  wrongProfile?.label ?? wrongEmail(unpushed[0] ?? first!),
                  repoLabel,
                )}
              </Banner>
            ) : null}
            {pushed.length ? (
              <Banner
                kind="warning"
                action={{ label: S.fix.pushedHelp, onClick: () => void openDocs() }}
              >
                {S.fix.pushed(pushed.length)}
              </Banner>
            ) : null}

            {unpushed.length ? (
              <CommitTable
                id="fix-unpushed"
                heading={split ? S.fix.unpushedHeading(unpushed.length) : undefined}
                label={S.fix.tableLabel}
                columns={columns}
                rows={unpushed}
              />
            ) : null}
            {pushed.length ? (
              <CommitTable
                id="fix-pushed"
                heading={split ? S.fix.pushedHeading(pushed.length) : undefined}
                label={S.fix.pushedTableLabel}
                columns={columns}
                rows={pushed}
              />
            ) : null}

            {canFix ? (
              <>
                {plan.signed ? (
                  <Banner kind="warning" live={false}>
                    {S.fix.signed(plan.signed)}
                  </Banner>
                ) : null}
                <Banner kind="info" icon={Icons.shield} live={false}>
                  {S.fix.reassure}
                  {plan.rewrite_count ? (
                    <span className="mt-1 block text-small">
                      {S.fix.newIds(plan.rewrite_count)}
                    </span>
                  ) : null}
                  {plan.foreign ? (
                    <span className="mt-1 block text-small">{S.fix.foreign(plan.foreign)}</span>
                  ) : null}
                </Banner>
                <div className="flex flex-wrap justify-end gap-2">
                  <Button onClick={() => setRepo(null)} disabled={fixing}>
                    {S.common.cancel}
                  </Button>
                  <Button variant="primary" onClick={apply} loading={fixing}>
                    {fixing ? S.fix.fixing : S.fix.fixWith(targetProfile?.label ?? '')}
                  </Button>
                </div>
              </>
            ) : null}
          </>
        )}
      </div>
    );
  }

  return (
    <div>
      {header}
      {body}
    </div>
  );
}

/** One commit table, with a visible heading when the plan has both kinds. */
function CommitTable({
  id,
  heading,
  label,
  columns,
  rows,
}: {
  id: string;
  heading?: string;
  label: string;
  columns: Column<OffendingCommit>[];
  rows: OffendingCommit[];
}) {
  const table = <Table label={label} columns={columns} rows={rows} rowKey={(c) => c.sha} />;
  if (!heading) return table;
  return (
    <section aria-labelledby={id} className="flex flex-col gap-2">
      <h2 id={id} className="text-body font-medium text-ink">
        {heading}
      </h2>
      {table}
    </section>
  );
}
