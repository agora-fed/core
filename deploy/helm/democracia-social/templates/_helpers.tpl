{{- define "ds.fullname" -}}
{{- printf "%s-%s" .Release.Name "gateway" | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "ds.labels" -}}
app.kubernetes.io/name: democracia-social
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/managed-by: Helm
app.kubernetes.io/part-of: pindorama
{{- end -}}

{{- define "ds.image" -}}
{{- $tag := .Values.image.tag | default .Chart.AppVersion -}}
{{- printf "%s/%s-gateway:%s" .Values.image.registry .Values.image.repository $tag -}}
{{- end -}}
