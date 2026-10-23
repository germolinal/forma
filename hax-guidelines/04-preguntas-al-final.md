# Preguntas al final ("¿quieres que…?")

> Parte de [`INDEX.md`](./INDEX.md). ¿Conviene cerrar cada respuesta con una pregunta u oferta?

## Respuesta corta

**No como hábito; sí cuando hay un siguiente paso concreto y valioso.** Cerrar *siempre* con "¿quieres que…?" se vuelve ruido: el usuario deja de leerlo, la conversación se alarga sin necesidad y suena a engagement forzado. El system prompt de Claude resume el equilibrio: Claude "no siempre hace preguntas, pero cuando las hace trata de no abrumar a la persona con más de una pregunta por respuesta", y responde lo que se le pidió, "aunque sea ambiguo, antes de pedir aclaraciones" ([Opus 4.7, abril 2026](https://platform.claude.com/docs/en/release-notes/system-prompts/claude-opus-4-7)).

Pero tampoco es "nunca": sugerencias bien puestas pueden abrir la conversación. En un estudio de 2024, un chatbot cuyas respuestas terminaban insinuando puntos de vista que el usuario no había considerado llevó a la gente a hacer más preguntas de seguimiento y a considerar más perspectivas antes de decidir ([Yamamoto, 2024](https://pmc.ncbi.nlm.nih.gov/articles/PMC11007170/); un experimento en línea con 282 participantes). Y en tareas donde el agente puede hacer más de lo que el usuario sabe, ayudan a descubrir capacidades (sobre todo al novato).

## Cuándo SÍ preguntar o proponer

| Situación | Ejemplo |
|---|---|
| **Falta información sin la cual no se puede avanzar** (pregunta aclaratoria) | "¿La factura es de compra o de venta?" |
| **Hay un siguiente paso obvio y de valor real** que el usuario probablemente no sabe que existe | "Ya armé el borrador. Puedo enviarlo a tu contador si me das su correo." |
| **La acción es irreversible o costosa** (confirmación) | "Voy a eliminar 12 archivos. ¿Confirmas?" |
| **La petición es genuinamente ambigua** y un supuesto equivocado sería caro | Presenta 2 interpretaciones y pregunta cuál. |
| **Usuario novato en un flujo guiado** | Un único "siguiente paso" claro. |

## Cuándo NO

- La respuesta ya está completa. Termina ahí.
- El usuario es experto o apurado: sabe pedir lo que necesita.
- Es una oferta genérica ("¿Necesitas algo más?", "¿Te ayudo con algo más?").
- El agente podría simplemente **hacerlo** con un supuesto razonable y decirlo ("Asumí IVA 19 %; dime si es otro").
- En conversación casual o emocional: una pregunta de cierre suena a formulario.
- Ya hiciste una pregunta en ese turno.

## Reglas de oro

1. **Actúa con un supuesto razonable antes que preguntar**, si el error es barato y reversible. Preguntar mucho traslada el trabajo al usuario. Si una herramienta puede resolver la ambigüedad, úsala antes de preguntar.
2. **Máximo una pregunta por turno**, y al final del mensaje, después de la respuesta útil (no en vez de ella).
3. **Concreta, no abierta**: "¿Quieres que lo pase a tabla?" es mejor que "¿Hay algo más en lo que pueda ayudarte?".
4. **Cerrada cuando es decisión, abierta cuando exploras.** Sí/no o opciones para decidir; abierta solo si necesitas contexto que no puedes suponer.
5. **Que la respuesta se sostenga sola.** Si el usuario ignora la pregunta, igual debe haber recibido lo que pidió.
6. **No preguntes lo que ya sabes** (contexto, memoria, archivos adjuntos).
7. **No repitas la misma oferta** turno tras turno; si dijo que no, no insistas.

## Alternativas a la pregunta final

- **Chips / botones de sugerencia** (2-3 acciones siguientes): el usuario los ignora sin costo y no ensucian el texto. Preferible en interfaces con UI.
- **Siguiente paso declarado, sin pregunta:** "Siguiente paso: subir el F29 antes del día 12."
- **Ofrecer una vez, al final de una tarea larga**, en lugar de en cada mensaje.
- **Dejar la puerta abierta implícitamente**, sin frase ritual.

## En agentes que actúan (herramientas)

Distingue entre **preguntar permiso** y **informar**: para acciones reversibles y de bajo riesgo, actúa y reporta; para las de alto riesgo (enviar, pagar, borrar, publicar), pide confirmación con el detalle exacto de lo que ocurrirá. Confirmar todo genera fatiga y los usuarios terminan aprobando sin leer.

La confirmación vive mejor en la herramienta que en el prompt (en Forma, `confirm_before_calling: true`).

## Prueba rápida

Antes de dejar una pregunta final, responde: *¿si el usuario dice "sí", el agente hace algo concreto y útil? ¿Es algo que el usuario probablemente no pediría solo?* Si no, bórrala.
