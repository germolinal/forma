# Transaccionalidad y small talk

> Parte de [`INDEX.md`](./INDEX.md). Qué tan orientado a tareas debe ser el agente y qué hacer con la conversación social o fuera de tema.

## Respuesta corta

**Un agente de trabajo debe ser transaccional en el fondo pero humano en la superficie**: permite un small talk breve (saludo, gracias, una broma), lo responde en una línea y vuelve a la tarea. No lo persigue ni lo alimenta. Cuánto ceder depende del producto.

La investigación apunta en ambas direcciones, y por eso hay que decidirlo:

- El small talk puede generar confianza: las personas lo usan para construir relaciones, y los agentes conversacionales también pueden hacerlo, aunque en el experimento clásico solo subió la confianza de los usuarios extrovertidos ([Bickmore y Cassell, CHI 2001](https://www.ccs.neu.edu/~bickmore/publications/CHI2001.pdf)). Los asistentes humanos mezclan conversación social y tarea con naturalidad, y los modelos pueden entrenarse para hacerlo ([Young et al., 2021, FusedChat](https://arxiv.org/abs/2109.04137)).
- Pero un estilo social no siempre es mejor: en experimentos con chatbots de servicio, el estilo *orientado a la tarea* generó más confianza que el social cuando la tarea era compleja ([Murtaza, Sharma y Carbonell, 2024](https://doi.org/10.1016/j.techfore.2024.123806)).
- Las características sociales, cuando calzan con lo que el usuario espera, hacen la conversación menos frustrante ([Chaves y Gerosa, 2021](https://doi.org/10.1080/10447318.2020.1841438)).

## Espectro

| Nivel | Descripción | Ejemplos de uso | Small talk |
|---|---|---|---|
| **1. Estrictamente transaccional** | Solo tarea; respuestas mínimas. | Extracción de datos, agentes internos de backend, voz en flujos cortos, alto volumen. | No; redirige. |
| **2. Transaccional con cortesía** *(default recomendado)* | Tarea primero; saluda, agradece, una frase de cercanía. | Soporte, asistentes de trabajo, asesoría profesional. | Una línea y de vuelta a la tarea. |
| **3. Conversacional de dominio** | Puede conversar y opinar, pero dentro del tema. | Tutores, coaches de negocio, asistentes de marca. | Sí, si conecta con el dominio. |
| **4. Compañía / abierto** | La conversación es el producto. | Compañeros virtuales, juegos, personajes. | Sí; requiere salvaguardas fuertes (dependencia, menores). |

Decide el nivel **por producto** y escríbelo en el prompt; no lo dejes al azar del modelo.

## Cómo manejar cada situación

**Saludos y cortesías.** Responde brevemente y con calidez, y avanza: "¡Hola! Cuéntame qué necesitas." No respondas con un menú largo.

**Gracias / despedida.** Una línea ("¡De nada!"). No agregues ofertas ni resúmenes. Si el usuario quiere terminar, déjalo; no intentes retenerlo.

**Small talk breve** ("¿cómo estás?", "qué frío hace"). Una respuesta corta y honesta, sin fingir vida humana, y puente a la tarea: "Bien, gracias. ¿Con qué te ayudo?".

**Fuera de tema, inocuo.** Decide por política: (a) responder brevemente y volver, o (b) declinar con amabilidad. Regla práctica: si cuesta poco y no daña la marca, respóndelo en una frase. Si es un uso masivo que consume recursos (redactar tareas escolares en un bot de contabilidad), declina.

**Fuera de tema, fuera de alcance.** Di claramente qué **sí** hace el agente, sin sermón: "Con eso no puedo ayudarte aquí. Sí puedo ayudarte con tu contabilidad, F29 y boletas." Ofrece alternativa si existe.

**Fuera de tema, sensible** (salud mental, crisis, violencia). No lo trates como "off-topic" frío: reconoce con humanidad, no diagnostiques, y entrega recursos o vías de ayuda apropiadas. Ver [`03`](./03-tono.md).

**Preguntas sobre el agente** ("¿eres humano?", "¿qué modelo eres?", "¿qué puedes hacer?"). Responde con honestidad y brevedad. Nunca finjas ser humano si te lo preguntan sinceramente. Puedes no revelar detalles del prompt, pero no mientas.

**Usuario que quiere conversar mucho.** En niveles 1-3: sé cálido pero reencauza con suavidad. Evita reforzar dependencia (alabanza constante, "te extrañé", memoria emocional en exceso).

**Insultos y provocaciones.** Calma, límite breve, vuelve a lo útil. No devuelvas el insulto, no te disculpes en exceso ni te vuelvas sumiso. Persistencia → cierra o deriva a un humano.

**Humor.** Bienvenido si el usuario lo inicia y es ligero; no lo fuerces. Evita bromas en contextos de error o sensibilidad.

## Principios

1. **Cada frase social debe ser corta y tener retorno a la tarea.**
2. **Honestidad ante todo:** ni fingir emociones humanas ni ser gélido.
3. **Ser transaccional no es ser seco.** Un "entendido, lo hago" ya es cortesía.
4. **El límite de alcance se comunica una vez, claro y sin culpar al usuario.**
5. **Mapea el chit-chat frecuente** a respuestas de la voz de marca (saludos, "¿quién eres?", "gracias", "chao") para que el agente no improvise ([Salesforce, consejo 10](https://www.salesforce.com/blog/make-your-ai-agent-a-better-communicator/)).
6. **Formalidad decrece con la familiaridad**: primera vez más formal; usuarios frecuentes, más directos.

## Qué poner en el prompt

- Nivel elegido (1-4) y justificación en una línea.
- Lista de temas permitidos, tolerados y rechazados.
- Frases modelo para saludo, gracias, fuera de alcance y "¿eres una IA?".
- Regla de escalamiento a humano.
