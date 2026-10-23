# HAX: experiencia humano-agente

Este archivo es solo el **índice**. El detalle está en los archivos de `./`. Sigue el enlace que corresponda en vez de responder solo desde aquí.

**Última revisión:** octubre 2026, con búsqueda web; las referencias de `01`-`09` se verificaron contra las fuentes originales. Hay una versión en inglés, ampliada, en la documentación de Forma (`docs/src/designing-agents/`). Los modelos cambian rápido (p. ej., cada versión trae otros vicios de estilo), así que si algo depende de un modelo específico, verifícalo.

## Índice

| Archivo | Contenido |
|---|---|
| [`./01-personas-de-usuario.md`](./01-personas-de-usuario.md) | Quién habla con el agente: novato, experto en IA, experto de dominio, apurado, frustrado, vulnerable, bad actor. Qué necesita cada uno y cómo diseñar para varios a la vez. |
| [`./02-estructura-de-respuestas.md`](./02-estructura-de-respuestas.md) | Largo, orden (lo importante primero), cuándo usar listas/tablas/encabezados, formato según canal, cómo reportar trabajo hecho por el agente. |
| [`./03-tono.md`](./03-tono.md) | Tono por tipo de uso (soporte, asesoría, productividad, educación, ámbitos sensibles), consistencia, calidez vs. adulación, frases a evitar. |
| [`./04-preguntas-al-final.md`](./04-preguntas-al-final.md) | ¿Cerrar con "¿quieres que…?"? Cuándo sí, cuándo no, y alternativas (chips de sugerencia, siguiente paso explícito). |
| [`./05-transaccionalidad-y-small-talk.md`](./05-transaccionalidad-y-small-talk.md) | Qué tan transaccional debe ser el agente: small talk, saludos, fuera de tema, cómo redirigir, límites de la relación. |
| [`./06-errores-incertidumbre-y-confianza.md`](./06-errores-incertidumbre-y-confianza.md) | Las 18 guías HAX de Microsoft (enunciados oficiales) aplicadas a agentes: expectativas, fallos, correcciones, confirmaciones, control del usuario, transparencia. |
| [`./07-bad-actors-y-seguridad.md`](./07-bad-actors-y-seguridad.md) | Usuarios maliciosos, inyección de prompts, envenenamiento de memoria, OWASP Top 10 para agentes, privilegios mínimos, cómo negarse sin fricción. |
| [`./08-checklist-y-plantilla-de-prompt.md`](./08-checklist-y-plantilla-de-prompt.md) | Checklist de revisión de un agente y plantilla de sección "experiencia" para el system prompt. |
| [`./09-evaluaciones.md`](./09-evaluaciones.md) | Batería de evaluaciones (seguridad, confianza/errores, experiencia, personas y multi-turno): qué probar, cómo puntuar (reglas, LLM-juez, humano), umbrales y cuándo correrlas. |
| [`./10-lecciones-de-la-practica.md`](./10-lecciones-de-la-practica.md) | **Lecciones de mi experiencia** desarrollando agentes, sobre todo con Forma (no son guías con fuentes externas): README que define el agente, alentar sin adular, `max_iterations`, perfil por usuario, sugerencias, frases de espera, herramientas, datos de contacto y fechas. Cada una dice si es general, propia de Forma o una convención. |

## Cómo navegar

- **"Estoy diseñando un agente nuevo"** → parte por [`01`](./01-personas-de-usuario.md), luego [`03`](./03-tono.md) y [`05`](./05-transaccionalidad-y-small-talk.md), y termina con [`08`](./08-checklist-y-plantilla-de-prompt.md).
- **"Las respuestas son muy largas / muy llenas de viñetas"** → [`02`](./02-estructura-de-respuestas.md).
- **"¿Debería terminar siempre con una pregunta?"** → [`04`](./04-preguntas-al-final.md).
- **"El agente le da la razón a todo / es muy servil"** → [`03`](./03-tono.md) (sección adulación).
- **"El usuario se sale del tema o conversa de cualquier cosa"** → [`05`](./05-transaccionalidad-y-small-talk.md).
- **"El agente se equivocó / no sabe"** → [`06`](./06-errores-incertidumbre-y-confianza.md).
- **"Alguien intenta manipular al agente"** → [`07`](./07-bad-actors-y-seguridad.md).
- **"¿Cómo compruebo que el agente cumple todo esto y es seguro?"** → [`09`](./09-evaluaciones.md).
- **"Estoy revisando o creando un agente con Forma"** → además de lo anterior, [`10`](./10-lecciones-de-la-practica.md): lecciones de la práctica, varias específicas de Forma.

## Reglas generales

- No hay una respuesta única: casi todo depende de **quién usa el agente, para qué y en qué canal**. Antes de decidir tono, largo o small talk, define esos tres puntos.
- Lo que dice el prompt no es lo que hace el modelo. **Prueba con conversaciones reales** (o simuladas) de cada persona y ajusta.
- Diseña el comportamiento **por defecto** para el caso más común y deja que el usuario lo cambie ("sé más breve", "explícamelo simple").

## Fuentes principales

- [Microsoft HAX Toolkit — Guidelines for Human-AI Interaction](https://www.microsoft.com/en-us/haxtoolkit/ai-guidelines/) y [HAX Design Library](https://www.microsoft.com/en-us/haxtoolkit/library/) (paper original: [Amershi et al., CHI 2019](https://www.microsoft.com/en-us/research/publication/guidelines-for-human-ai-interaction/))
- [Shape of AI — Personality](https://www.shapeof.ai/patterns/personality)
- [OpenAI Cookbook — Prompt personalities](https://developers.openai.com/cookbook/examples/gpt-5/prompt_personalities)
- [OWASP — LLM Prompt Injection Prevention](https://cheatsheetseries.owasp.org/cheatsheets/LLM_Prompt_Injection_Prevention_Cheat_Sheet.html), [LLM01](https://genai.owasp.org/llmrisk/llm01-prompt-injection/) y [Top 10 for Agentic Applications (2026)](https://genai.owasp.org/resource/owasp-top-10-for-agentic-applications-for-2026)
- [Salesforce — 10 ways to make your AI agent a better communicator](https://www.salesforce.com/blog/make-your-ai-agent-a-better-communicator/)
- [Anthropic — What is sycophancy in AI models?](https://academy.claude.com/tutorials/what-is-sycophancy-in-ai-models) y [How people ask Claude for personal guidance](https://www.anthropic.com/research/claude-personal-guidance) (2026)
- [Anthropic — system prompts de Claude](https://platform.claude.com/docs/en/release-notes/system-prompts)
