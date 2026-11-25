import { tagSha } from './flux.ts';

export type Stage =
  | 'BUILDING'
  | 'BUILD_FAILED'
  | 'SCAN_PENDING'
  | 'SELECTED'
  | 'COMMITTED'
  | 'ROLLING_OUT'
  | 'DEGRADED'
  | 'LIVE';

export interface Build {
  number: number;
  sha: string;
  status: string;
  stoppedAt?: string;
  failedJob?: string;
}

export interface DeliveryInput {
  selectedTag?: string;
  desiredTag?: string;
  runningTags: string[];
  lastScanAt?: string;
  build?: Build;
  workloads: {
    name: string;
    ready: number;
    desired: number;
    waitingReason?: string;
  }[];
}

export interface Delivery {
  stage: Stage;
  blocker?: string;
}

const IN_PROGRESS = new Set(['running', 'waiting', 'blocked']);
const FAILED = new Set(['failure', 'cancelled']);

// walks the chain from the newest build towards the running pods and stops at the first gap
export function delivery(input: DeliveryInput): Delivery {
  const { build, selectedTag, desiredTag, runningTags } = input;
  const running = runningTags.map(tagSha);
  const built = build?.sha.slice(0, 7);

  if (build && built && !running.includes(built)) {
    if (IN_PROGRESS.has(build.status)) {
      return {
        stage: 'BUILDING',
        blocker: `build run #${build.number} is ${build.status}`,
      };
    }
    if (FAILED.has(build.status)) {
      const where = build.failedJob ? ` in ${build.failedJob}` : '';
      return {
        stage: 'BUILD_FAILED',
        blocker: `build run #${build.number} ${build.status}${where}`,
      };
    }
    if (build.status === 'success' && tagSha(selectedTag) !== built) {
      const scannedBefore =
        build.stoppedAt &&
        input.lastScanAt &&
        Date.parse(input.lastScanAt) < Date.parse(build.stoppedAt);
      return {
        stage: 'SCAN_PENDING',
        blocker: scannedBefore
          ? `${built} was released at ${build.stoppedAt}, after the image repository last scanned at ${input.lastScanAt}`
          : `${built} was released but the image policy still selects ${selectedTag ?? 'nothing'}`,
      };
    }
  }

  if (selectedTag && desiredTag !== selectedTag) {
    return {
      stage: 'SELECTED',
      blocker: `image automation hasn't committed ${selectedTag} yet; git has ${desiredTag ?? 'no tag'}`,
    };
  }

  if (desiredTag && !runningTags.includes(desiredTag)) {
    return {
      stage: 'COMMITTED',
      blocker: `git has ${desiredTag}, the workloads run ${runningTags.join(', ') || 'nothing'}`,
    };
  }

  const waiting = input.workloads.find((workload) => workload.waitingReason);
  if (waiting) {
    return {
      stage: 'DEGRADED',
      blocker: `${waiting.name}: ${waiting.waitingReason}`,
    };
  }

  const behind = input.workloads.find((workload) => workload.ready < workload.desired);
  if (behind) {
    return {
      stage: 'ROLLING_OUT',
      blocker: `${behind.name}: ${behind.ready}/${behind.desired} ready`,
    };
  }

  return { stage: 'LIVE' };
}
