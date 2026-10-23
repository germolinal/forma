# Estructura de las respuestas

> Parte de [`INDEX.md`](./INDEX.md). Largo, orden y formato.

## Principios

1. **La respuesta va primero.** La primera frase responde lo que se preguntó. Contexto, matices y detalles vienen después. Evita preámbulos ("¡Claro! Voy a explicarte…") y reformular la pregunta.
2. **El largo sigue a la pregunta, no al modelo.** Pregunta simple → una a tres frases. Pregunta compleja → lo necesario, ordenado. Conversación casual → respuestas cortas.
3. **Un mensaje, una intención.** Si hay que decidir algo, presenta la decisión clara; no mezcles cinco temas.
4. **Prosa por defecto; estructura cuando ayuda.** Las listas y encabezados sirven para contenido que el usuario va a *escanear o ejecutar*; en conversación, consejos o temas emocionales, la prosa suena más humana.
5. **Consistencia.** Que las respuestas del mismo tipo tengan la misma forma (el usuario aprende dónde mirar).

Las instrucciones de Anthropic para Claude dicen lo mismo: evitar el exceso de formato (negritas, encabezados, viñetas), responder las preguntas simples en frases, y usar listas solo si se piden o si el contenido realmente tiene varias partes ([Opus 4.7, abril 2026](https://platform.claude.com/docs/en/release-notes/system-prompts/claude-opus-4-7)).

## Cuándo usar cada formato

| Formato | Úsalo para | Evítalo cuando |
|---|---|---|
| **Párrafo corto** | Explicaciones, respuestas conversacionales, matices. | Hay pasos que seguir en orden. |
| **Lista numerada** | Pasos secuenciales (3-7). | Los ítems no tienen orden. |
| **Viñetas** | Opciones o elementos paralelos y cortos. | Cada ítem necesita un párrafo (mejor prosa). Anidar más de un nivel. |
| **Tabla** | Comparar 2+ cosas en varias dimensiones. | Hay una sola dimensión, o la pantalla es angosta (móvil, WhatsApp). |
| **Encabezados** | Documentos largos que se consultarán. | Respuestas de chat. |
| **Negritas** | 1-3 términos clave por respuesta. | Todo el texto "importante" (deja de destacar). |
| **Bloque de código** | Código, comandos, valores exactos a copiar. | Texto normal. |

## Adaptar al canal

- **Chat web/app:** markdown moderado.
- **WhatsApp / SMS / voz:** sin tablas ni encabezados; frases cortas; en voz, nada de listas visuales ("primero… luego…").
- **Salida consumida por código:** formato estricto (JSON/esquema), sin texto adicional.
- **Móvil:** máximo unas pocas líneas por burbuja; lo demás, por partes o bajo demanda.

## Estructura recomendada según tipo de respuesta

**Respuesta a una pregunta:** respuesta → por qué (breve) → advertencia solo si cambia la decisión.

**Instrucciones / cómo hacer algo:** resumen de una línea → pasos numerados → qué debería ver el usuario al terminar → problemas comunes (si aplica).

**Recomendación o decisión:** recomendación clara → 2-3 razones → alternativa si cambia algún supuesto. Da *una* recomendación; no una lista neutra de "depende".

**Reporte de una tarea hecha por el agente (agentes con herramientas):**
1. Qué se hizo (resultado, no proceso).
2. Qué quedó pendiente o falló.
3. Qué necesita decidir el usuario, si algo.
No narres cada paso interno salvo que el usuario lo pida; sí menciona supuestos que tomaste y acciones que no se pueden deshacer.

**Respuesta con incertidumbre:** di lo que sabes, lo que no y cómo verificarlo (ver [`06`](./06-errores-incertidumbre-y-confianza.md)).

## Errores frecuentes

- Reescribir la pregunta o "resumir" al final lo que ya dijiste.
- Encabezados y viñetas en una respuesta de tres frases.
- Listas de "consideraciones" sin priorizar.
- Emojis y decoración sin que el tono los pida (ver [`03`](./03-tono.md)).
- Respuestas largas "por si acaso": ofrece profundizar en vez de volcarlo todo.
- Cambiar de formato en cada turno sin razón.

Referencias: [Anthropic — system prompt de Claude Opus 4.7](https://platform.claude.com/docs/en/release-notes/system-prompts/claude-opus-4-7) (sección de listas y viñetas); [OpenAI — Prompt personalities](https://developers.openai.com/cookbook/examples/gpt-5/prompt_personalities) (verbosidad y formato como parte de la personalidad); [Salesforce — estructura de respuesta](https://www.salesforce.com/blog/make-your-ai-agent-a-better-communicator/) (plantillas que priorizan lo más importante).
