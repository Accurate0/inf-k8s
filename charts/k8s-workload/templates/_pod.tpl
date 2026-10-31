{{- define "k8s-workload.pod" -}}
{{- $w := .workload }}
{{- $service := $w.service | default dict }}
{{- $tag := required (printf "no tag for image %s" $w.image) (get .images $w.image) }}
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
  restartPolicy: OnFailure
  {{- end }}
  securityContext:
    runAsNonRoot: true
    runAsUser: 10001
    runAsGroup: 10001
    seccompProfile:
      type: RuntimeDefault
  containers:
    - name: {{ $w.name }}
      image: {{ $w.image }}:{{ $tag }}
      imagePullPolicy: IfNotPresent
      securityContext:
        allowPrivilegeEscalation: false
        readOnlyRootFilesystem: {{ ne $w.readOnlyRootFilesystem false }}
        capabilities:
          drop:
            - ALL
      resources:
        {{- toYaml $w.resources | nindent 8 }}
      {{- with $w.port }}
      ports:
        - name: {{ $service.portName | default "http" }}
          containerPort: {{ . }}
      {{- end }}
      {{- with $w.probes }}
      {{- $port := .port | default $w.port }}
      readinessProbe:
        httpGet:
          path: {{ .path }}
          port: {{ $port }}
        periodSeconds: 10
      livenessProbe:
        httpGet:
          path: {{ .path }}
          port: {{ $port }}
        initialDelaySeconds: 3
        periodSeconds: 30
      {{- end }}
      {{- with $w.env }}
      env:
        {{- toYaml . | nindent 8 }}
      {{- end }}
      {{- with $w.envFrom }}
      envFrom:
        {{- toYaml . | nindent 8 }}
      {{- end }}
{{- end }}
