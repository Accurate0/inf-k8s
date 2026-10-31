{{- define "k8s-workload.container" -}}
{{- $c := .container }}
{{- $tag := required (printf "no tag for image %s" $c.image) (get .images $c.image) }}
- name: {{ $c.name }}
  image: {{ $c.image }}:{{ $tag }}
  imagePullPolicy: IfNotPresent
  {{- with $c.command }}
  command:
    {{- toYaml . | nindent 4 }}
  {{- end }}
  {{- with $c.args }}
  args:
    {{- toYaml . | nindent 4 }}
  {{- end }}
  {{- if ne .runAsNonRoot false }}
  securityContext:
    allowPrivilegeEscalation: false
    readOnlyRootFilesystem: {{ ne $c.readOnlyRootFilesystem false }}
    capabilities:
      drop:
        - ALL
  {{- end }}
  resources:
    {{- toYaml ($c.resources | default .resources) | nindent 4 }}
  {{- with $c.ports }}
  ports:
    {{- toYaml . | nindent 4 }}
  {{- end }}
  {{- with $c.startupProbe }}
  startupProbe:
    {{- toYaml . | nindent 4 }}
  {{- end }}
  {{- with $c.readinessProbe }}
  readinessProbe:
    {{- toYaml . | nindent 4 }}
  {{- end }}
  {{- with $c.livenessProbe }}
  livenessProbe:
    {{- toYaml . | nindent 4 }}
  {{- end }}
  {{- with $c.env }}
  env:
    {{- toYaml . | nindent 4 }}
  {{- end }}
  {{- with $c.envFrom }}
  envFrom:
    {{- toYaml . | nindent 4 }}
  {{- end }}
  {{- with $c.volumeMounts }}
  volumeMounts:
    {{- toYaml . | nindent 4 }}
  {{- end }}
{{- end }}

{{- define "k8s-workload.pod" -}}
{{- $w := .workload }}
{{- $service := $w.service | default dict }}
{{- $ports := $w.ports }}
{{- if and (not $ports) $w.port }}
{{- $ports = list (dict "name" ($service.portName | default "http") "containerPort" $w.port) }}
{{- end }}
{{- $probes := dict }}
{{- with $w.probes }}
{{- $port := .port | default $w.port }}
{{- $get := dict "path" .path "port" $port }}
{{- $probes = dict "readinessProbe" (dict "httpGet" $get "periodSeconds" 10) "livenessProbe" (dict "httpGet" $get "initialDelaySeconds" 3 "periodSeconds" 30) }}
{{- end }}
{{- $main := merge (dict "name" $w.name "ports" $ports) (omit $w "name" "ports" "probes") $probes }}
{{- $nonRoot := $w.runAsNonRoot }}
metadata:
  labels:
    app: {{ $w.name }}
  annotations:
    reloader.stakater.com/auto: "true"
    {{- with $w.annotations }}
    {{- toYaml . | nindent 4 }}
    {{- end }}
spec:
  {{- if eq $w.kind "CronJob" }}
  restartPolicy: {{ $w.restartPolicy | default "OnFailure" }}
  {{- end }}
  {{- with $w.serviceAccountName }}
  serviceAccountName: {{ . }}
  {{- end }}
  {{- if hasKey $w "automountServiceAccountToken" }}
  automountServiceAccountToken: {{ $w.automountServiceAccountToken }}
  {{- end }}
  {{- with $w.hostAliases }}
  hostAliases:
    {{- toYaml . | nindent 4 }}
  {{- end }}
  {{- with $w.affinity }}
  affinity:
    {{- toYaml . | nindent 4 }}
  {{- end }}
  {{- with $w.topologySpreadConstraints }}
  topologySpreadConstraints:
    {{- toYaml . | nindent 4 }}
  {{- end }}
  {{- if ne $nonRoot false }}
  securityContext:
    runAsNonRoot: true
    runAsUser: 10001
    runAsGroup: 10001
    seccompProfile:
      type: RuntimeDefault
  {{- end }}
  {{- with $w.volumes }}
  volumes:
    {{- toYaml . | nindent 4 }}
  {{- end }}
  {{- with $w.initContainers }}
  initContainers:
    {{- range . }}
    {{- include "k8s-workload.container" (dict "container" . "images" $.images "runAsNonRoot" $nonRoot "resources" $w.resources) | nindent 4 }}
    {{- end }}
  {{- end }}
  containers:
    {{- include "k8s-workload.container" (dict "container" $main "images" .images "runAsNonRoot" $nonRoot "resources" $w.resources) | nindent 4 }}
    {{- range $w.sidecars }}
    {{- include "k8s-workload.container" (dict "container" . "images" $.images "runAsNonRoot" $nonRoot "resources" $w.resources) | nindent 4 }}
    {{- end }}
{{- end }}
