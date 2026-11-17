export interface Rollout {
  kind: string;
  metadata: { generation?: number };
  spec: { replicas?: number };
  status?: RolloutStatus;
}

export interface RolloutStatus {
  observedGeneration?: number;
  replicas?: number;
  updatedReplicas?: number;
  availableReplicas?: number;
  readyReplicas?: number;
  currentRevision?: string;
  updateRevision?: string;
  desiredNumberScheduled?: number;
  updatedNumberScheduled?: number;
  numberAvailable?: number;
}

export const ROLLED_OUT: Record<string, (workload: Rollout, status: RolloutStatus) => boolean> = {
  Deployment: ({ spec }, status) =>
    status.updatedReplicas === (spec.replicas ?? 1) &&
    status.replicas === status.updatedReplicas &&
    status.availableReplicas === status.updatedReplicas,
  StatefulSet: ({ spec }, status) =>
    status.readyReplicas === (spec.replicas ?? 1) &&
    status.currentRevision === status.updateRevision,
  DaemonSet: (_, status) =>
    status.updatedNumberScheduled === status.desiredNumberScheduled &&
    status.numberAvailable === status.desiredNumberScheduled,
};

export function rolledOut(workload: Rollout): boolean {
  const status = workload.status ?? {};
  const check = ROLLED_OUT[workload.kind];
  return (
    !!check &&
    (status.observedGeneration ?? 0) >= (workload.metadata.generation ?? 0) &&
    check(workload, status)
  );
}
