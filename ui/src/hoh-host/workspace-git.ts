import type { VcsListRefsInput, VcsStatusResult } from '../t3/base/contracts/git.ts';
import type {
  ReviewBranchCommitsInput, ReviewDiffFileContentsInput, ReviewDiffPreviewInput,
} from '../t3/base/contracts/review.ts';
import { englishError } from './errors.ts';
import { RpcFailure, type StreamSink } from './wire.ts';

export type WorkspaceGitQuery =
  | { action: 'status' }
  | ({ action: 'listRefs' } & Omit<VcsListRefsInput, 'cwd'>)
  | ({ action: 'diffPreview' } & Omit<ReviewDiffPreviewInput, 'cwd'>)
  | ({ action: 'branchCommits' } & Omit<ReviewBranchCommitsInput, 'cwd'>)
  | ({ action: 'diffFileContents' } & Omit<ReviewDiffFileContentsInput, 'cwd'>);

const NO_REPO: VcsStatusResult = {
  isRepo: false, hasPrimaryRemote: false, isDefaultRef: false, refName: null,
  hasWorkingTreeChanges: false, workingTree: { files: [], insertions: 0, deletions: 0 },
  hasUpstream: false, aheadCount: 0, behindCount: 0, pr: null,
};

const STATUS_POLL_MS = 5000;
const toGitFailure = (cwd: string, operation: string, error: unknown) =>
  new RpcFailure({
    _tag: 'GitCommandError', operation, command: 'git', cwd,
    detail: englishError(error, 'Unable to read the Git project.'),
  });

function snapshot({
  hasUpstream, aheadCount, behindCount, aheadOfDefaultCount, pr, ...local
}: VcsStatusResult) {
  return {
    _tag: 'snapshot', local,
    remote: local.isRepo ? { hasUpstream, aheadCount, behindCount, aheadOfDefaultCount, pr } : null,
  };
}

interface StatusObserver {
  sink: StreamSink;
  branchOnly: boolean;
  last?: string;
}
interface StatusPoll {
  observers: Set<StatusObserver>;
  timer?: ReturnType<typeof setTimeout>;
}

// Both of T3's checkout subscriptions share the same native status read. Recheck
// while observed so an edit made outside HoH also reaches the Changes panel.
export class WorkspaceGit {
  private readonly pending = new Map<string, Promise<VcsStatusResult>>();
  private readonly polls = new Map<string, StatusPoll>();

  constructor(
    private readonly query: (cwd: string, query: WorkspaceGitQuery) => Promise<unknown>,
  ) {}

  private status(cwd: string): Promise<VcsStatusResult> {
    const existing = this.pending.get(cwd);
    if (existing) return existing;
    const reading = this.query(cwd, { action: 'status' }).then(
      result => (result as VcsStatusResult | null) ?? NO_REPO,
      error => { throw toGitFailure(cwd, 'status', error); },
    ).finally(() => this.pending.delete(cwd));
    this.pending.set(cwd, reading);
    return reading;
  }

  private notify(cwd: string, status: VcsStatusResult) {
    for (const observer of this.polls.get(cwd)?.observers ?? []) {
      const value = observer.branchOnly ? { refName: status.refName, isRepo: status.isRepo } : snapshot(status);
      const key = JSON.stringify(value);
      if (observer.last !== key) {
        observer.last = key;
        observer.sink.emit(value);
      }
    }
  }

  async refresh(input: { cwd: string }) {
    const status = await this.status(input.cwd);
    this.notify(input.cwd, status);
    return status;
  }

  subscribe(input: { cwd: string }, sink: StreamSink, branchOnly = false): () => void {
    const cwd = input.cwd;
    let poll = this.polls.get(cwd);
    if (!poll) {
      poll = { observers: new Set() };
      this.polls.set(cwd, poll);
    }
    const current = poll;
    const observer: StatusObserver = { sink, branchOnly };
    const first = !poll.observers.size;
    poll.observers.add(observer);
    const stop = () => {
      current.observers.delete(observer);
      if (!current.observers.size && this.polls.get(cwd) === current) {
        if (current.timer !== undefined) clearTimeout(current.timer);
        this.polls.delete(cwd);
      }
    };
    const read = async () => {
      try {
        await this.refresh(input);
      } catch (error) {
        if (current.observers.has(observer) && observer.last === undefined) {
          stop();
          sink.fail(error);
        }
      }
    };
    if (first) {
      const tick = async () => {
        await read();
        if (this.polls.get(cwd) === current) current.timer = setTimeout(tick, STATUS_POLL_MS);
      };
      void tick();
    } else {
      void read();
    }
    return stop;
  }

  stop(): void {
    for (const poll of this.polls.values()) {
      if (poll.timer !== undefined) clearTimeout(poll.timer);
      poll.observers.clear();
    }
    this.polls.clear();
  }

  async listRefs({ cwd, ...input }: VcsListRefsInput) {
    return this.run(cwd, { action: 'listRefs', ...input }, {
      isRepo: false, hasPrimaryRemote: false, refs: [], nextCursor: null, totalCount: 0,
    });
  }

  async diffPreview({ cwd, ...input }: ReviewDiffPreviewInput) {
    const result = await this.run(cwd, { action: 'diffPreview', ...input }, { cwd, sources: [] });
    return { ...result, generatedAt: new Date().toISOString() };
  }

  branchCommits({ cwd, ...input }: ReviewBranchCommitsInput) {
    return this.run(cwd, { action: 'branchCommits', ...input }, { commits: [], truncated: false });
  }

  async fileContents({ cwd, ...input }: ReviewDiffFileContentsInput) {
    const result = await this.run(cwd, { action: 'diffFileContents', ...input }, null);
    if (result === null) throw toGitFailure(cwd, 'diffFileContents', 'The project folder is not a Git checkout.');
    return result;
  }

  private async run<T>(cwd: string, query: WorkspaceGitQuery, fallback: T): Promise<T> {
    try {
      return (await this.query(cwd, query) as T | null) ?? fallback;
    } catch (error) {
      throw toGitFailure(cwd, query.action, error);
    }
  }
}
