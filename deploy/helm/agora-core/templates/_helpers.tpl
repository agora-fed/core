{{- define "agora.fullname" -}}
{{- printf "%s-%s" .Release.Name "gateway" | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "agora.labels" -}}
app.kubernetes.io/name: agora-core
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/managed-by: Helm
app.kubernetes.io/part-of: agora
{{- end -}}

{{- define "agora.selectorLabels" -}}
app.kubernetes.io/name: agora-core
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{- define "agora.image" -}}
{{- $tag := .Values.image.tag | default .Chart.AppVersion -}}
{{- printf "%s/%s:%s" .Values.image.registry .Values.image.repository $tag -}}
{{- end -}}
