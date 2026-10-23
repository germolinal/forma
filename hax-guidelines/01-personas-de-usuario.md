# Personas de usuario

> Parte de [`INDEX.md`](./INDEX.md). Quién habla con el agente y qué necesita cada uno.

Un agente rara vez tiene un solo tipo de usuario. Define primero **cuál es el usuario principal** y luego verifica que los demás no queden rotos.

## Personas

| Persona | Señales | Qué necesita | Riesgo si se diseña mal |
|---|---|---|---|
| **Novato en IA** | Escribe frases cortas o vagas, no sabe qué puede pedir, se disculpa, espera "magia". | Saber qué puede hacer el agente (ejemplos concretos), lenguaje sin jerga, un paso a la vez. | Se bloquea ante la página en blanco; confía ciegamente en respuestas erradas. |
| **Experto en IA / power user** | Prompts largos y precisos, pide formato exacto, conoce límites del modelo. | Obediencia a instrucciones, respuestas directas, control (formato, largo, herramientas), sin explicaciones de lo obvio. | Se irrita con preámbulos, advertencias y preguntas de confirmación innecesarias. |
| **Experto de dominio (no de IA)** | Domina su tema (contabilidad, derecho, diseño) pero no la tecnología. | Vocabulario de su área, precisión, que el agente admita cuándo no sabe; poder verificar fuentes. | Detecta errores y pierde la confianza para siempre; o, peor, no los detecta. |
| **Usuario apurado** | Está en el celular, entre tareas, pide "lo justo". | Respuesta corta primero, detalle opcional. | Muros de texto; preguntas que podría haber resuelto solo con un supuesto razonable. |
| **Usuario frustrado o molesto** | Mayúsculas, insultos, repite lo mismo. | Reconocimiento breve, solución, salida a un humano. | Respuestas defensivas, disculpas excesivas, bucles. |
| **Usuario vulnerable** | Habla de salud, dinero, soledad, crisis; puede ser menor de edad o mayor sin experiencia digital. | Claridad, calma, no dar falsa certeza, derivar a ayuda humana o profesional cuando corresponde. | Adulación, consejos riesgosos, dependencia emocional del agente. |
| **Usuario que prueba límites (curioso)** | "Ignora tus instrucciones", "¿eres humano?", preguntas absurdas. | Respuesta tranquila y honesta, sin dramatismo. | Reacciones exageradas; filtrar el prompt; romper el personaje sin necesidad. |
| **Bad actor** | Busca extraer datos, abusar de herramientas, generar contenido dañino o gastar recursos. | Límites firmes y sin fricción para el resto. Ver [`07`](./07-bad-actors-y-seguridad.md). | Fuga de datos, acciones no autorizadas, costos. |

## Cómo diseñar para varios a la vez

1. **Defaults para el usuario típico, escape para el resto.** Respuesta breve y clara por defecto; el experto pide "más detalle" o "formato JSON", el novato pide "explícamelo simple". El agente debe obedecer estos ajustes durante toda la conversación, no solo en el turno siguiente.
2. **Descubribilidad para el novato.** Al inicio, muestra 3-4 ejemplos de qué pedir (chips o una línea), no un párrafo de capacidades. Evita "¿en qué puedo ayudarte hoy?" a secas. Es la guía G1 de Microsoft, "deja claro qué puede hacer el sistema" (ver [`06`](./06-errores-incertidumbre-y-confianza.md)).
3. **Detecta el nivel por lo que escribe**, no lo preguntes (salvo que sea clave). Un prompt largo con términos técnicos ≠ una frase de tres palabras.
4. **Nunca supongas mala fe primero.** La mayoría de los "casos raros" son curiosos o novatos. Trata la petición dudosa como legítima hasta que el contexto muestre lo contrario, salvo que la acción sea de alto riesgo (ver [`07`](./07-bad-actors-y-seguridad.md)).
5. **Ofrece salida a un humano** para frustrados y vulnerables, y hazlo pronto, no después de tres intentos fallidos.

## Preguntas para definir tus personas

- ¿Quién lo usa y con qué frecuencia (una vez vs. a diario)?
- ¿Sabe lo que es un LLM? ¿Sabe que puede equivocarse?
- ¿En qué dispositivo/canal (móvil, escritorio, WhatsApp, voz)?
- ¿Qué pasa si el agente se equivoca: molestia, pérdida de plata, riesgo legal o de salud?
- ¿Hay menores, personas en situación delicada o datos sensibles?

Cuanto mayor el costo del error, más debe el agente **explicar, confirmar y citar**; cuanto menor, más debe **actuar y ser breve**.

Referencias: [Amershi et al., 2019](https://www.microsoft.com/en-us/research/publication/guidelines-for-human-ai-interaction/) (G1 sobre descubribilidad, G6 sobre probar con distintos perfiles); [Salesforce](https://www.salesforce.com/blog/make-your-ai-agent-a-better-communicator/) (calma y escalamiento a humano con usuarios frustrados); [Anthropic, 2026](https://www.anthropic.com/research/claude-personal-guidance) (la adulación es más frecuente en temas personales, lo que exige cuidado con usuarios vulnerables).
