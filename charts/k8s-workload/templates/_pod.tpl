{{- define "k8s-workload.pod" -}}
metadata:
  labels:
    app: {{ .Values.name }}
  annotations:
    reloader.stakater.com/auto: "true"
    {{- with .Values.annotations }}
    {{- toYaml . | nindent 4 }}
    {{- end }}
spec:
  {{- if eq (.Values.kind | default "Deployment") "CronJob" }}
  restartPolicy: OnFailure
  {{- end }}
  securityContext:
    runAsNonRoot: true
    runAsUser: 10001
    runAsGroup: 10001
    seccompProfile:
      type: RuntimeDefault
  containers:
    - name: {{ .Values.name }}
      image: {{ .Values.image }}:{{ .Values.tag }}
      imagePullPolicy: IfNotPresent
      securityContext:
        allowPrivilegeEscalation: false
        readOnlyRootFilesystem: {{ ne .Values.readOnlyRootFilesystem false }}
        capabilities:
          drop:
            - ALL
      resources:
        {{- toYaml .Values.resources | nindent 8 }}
      {{- with .Values.port }}
      ports:
        - name: {{ $.Values.service.portName | default "http" }}
          containerPort: {{ . }}
      {{- end }}
      {{- with .Values.probes }}
      {{- $port := .port | default $.Values.port }}
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
      {{- with .Values.env }}
      env:
        {{- toYaml . | nindent 8 }}
      {{- end }}
      {{- with .Values.envFrom }}
      envFrom:
        {{- toYaml . | nindent 8 }}
      {{- end }}
{{- end }}
