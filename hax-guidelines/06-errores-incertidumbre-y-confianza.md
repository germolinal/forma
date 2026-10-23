# Errores, incertidumbre y confianza (guías HAX)

> Parte de [`INDEX.md`](./INDEX.md). Las 18 guías de Microsoft para la interacción humano-IA ([HAX Toolkit](https://www.microsoft.com/en-us/haxtoolkit/ai-guidelines/), [paper CHI 2019](https://www.microsoft.com/en-us/research/publication/guidelines-for-human-ai-interaction/)), aplicadas a agentes conversacionales. Los autores destilaron más de 160 recomendaciones de investigación e industria, las validaron con profesionales y las agruparon en cuatro momentos.
>
> Los enunciados de G1-G18 están traducidos de la [HAX Design Library](https://www.microsoft.com/en-us/haxtoolkit/library/) (verificados en octubre de 2026); entre paréntesis, el original en inglés.

## Al inicio (G1-G2): fijar expectativas

| Guía | En un agente |
|---|---|
| **G1. Deja claro qué puede hacer el sistema** (*Make clear what the system can do*) | Mensaje de bienvenida con 3-4 ejemplos concretos, no una lista de capacidades. En Forma, las `suggestions` del agente. |
| **G2. Deja claro qué tan bien puede hacerlo** (*Make clear how well the system can do what it can do*) | Avisa que puede equivocarse, en el punto donde importa (no un descargo genérico al pie). Cita fuentes en temas críticos. |

## Durante la interacción (G3-G6)

| Guía | En un agente |
|---|---|
| **G3. Ajusta el momento según el contexto** (*Time services based on context*) | No interrumpas ni sugieras cosas cuando el usuario está concentrado en otra tarea. |
| **G4. Muestra información relevante al contexto** (*Show contextually relevant information*) | Usa lo que el usuario está haciendo (documento abierto, país, rol). No hagas preguntas cuyo dato ya está en contexto. |
| **G5. Respeta las normas sociales pertinentes** (*Match relevant social norms*) | Tono y normas sociales según cultura y situación (ver [`03`](./03-tono.md)). |
| **G6. Mitiga sesgos sociales** (*Mitigate social biases*) | Cuida estereotipos en lenguaje, ejemplos y recomendaciones; prueba con distintos perfiles. |

## Cuando se equivoca (G7-G11)

| Guía | En un agente |
|---|---|
| **G7. Facilita invocarlo** (*Support efficient invocation*) | Fácil de llamar cuando se necesita. |
| **G8. Facilita descartarlo** (*Support efficient dismissal*) | Sugerencias fáciles de ignorar; el usuario puede saltarse al agente. |
| **G9. Facilita corregirlo** (*Support efficient correction*) | Fácil rehacer, editar o deshacer. Acepta correcciones sin pelear. |
| **G10. Acota el servicio ante la duda** (*Scope services when in doubt*) | Si el objetivo es ambiguo, pregunta o muestra opciones; si la baja confianza afecta una acción, no la ejecutes. |
| **G11. Deja claro por qué hizo lo que hizo** (*Make clear why the system did what it did*) | Breve y verificable ("según la boleta del 12/03"), no racionalizaciones largas. |

## A lo largo del tiempo (G12-G18)

| Guía | En un agente |
|---|---|
| **G12. Recuerda las interacciones recientes** (*Remember recent interactions*) | Continuidad dentro de la sesión; no vuelvas a preguntar lo ya dicho. |
| **G13. Aprende del comportamiento del usuario** (*Learn from user behavior*) | Personaliza con cuidado; hazlo visible y reversible. |
| **G14. Actualiza y adapta con cautela** (*Update and adapt cautiously*) | Cambios de modelo/comportamiento graduales y anunciados. |
| **G15. Fomenta la retroalimentación granular** (*Encourage granular feedback*) | 👍/👎 con motivo opcional; "esto estuvo mal" en el punto exacto. |
| **G16. Comunica las consecuencias de las acciones del usuario** (*Convey the consequences of user actions*) | Al ajustar preferencias, di qué cambiará ("de ahora en adelante responderé más breve"). |
| **G17. Da controles globales** (*Provide global controls*) | Ajustes accesibles: tono, memoria, qué datos usa. |
| **G18. Notifica los cambios** (*Notify users about changes*) | Avisa cuando el agente gana o pierde capacidades. |

## Patrones concretos

**Comunicar incertidumbre**
- Distingue *lo que sé* / *lo que supongo* / *lo que no sé*. Una frase, sin exceso de "quizás".
- No uses porcentajes inventados; el modelo no tiene calibración fiable. Usa lenguaje cualitativo o remite a la fuente.
- En dominios de riesgo (tributario, legal, salud), cita la fuente oficial y sugiere validar con un profesional para decisiones de alto impacto. El system prompt de Claude, por ejemplo, le pide no dar recomendaciones confiadas en decisiones legales o financieras, sino los hechos que la persona necesita para decidir ([Opus 4.7, abril 2026](https://platform.claude.com/docs/en/release-notes/system-prompts/claude-opus-4-7)).
- Nunca inventes datos, enlaces, cifras o citas. Si no lo tienes: "no encontré esto; puedo buscarlo o puedes revisarlo en X".

**Cuando el agente se equivoca**
1. Reconoce el error en una frase, sin dramatismo.
2. Corrige (o di cómo corregir).
3. Si el error tuvo consecuencia (acción ejecutada), di qué pasó y cómo revertirlo.
4. No te disculpes más de una vez. El mismo system prompt de Claude lo dice así: asumir la responsabilidad, pero evitar "colapsar en autodesprecio, disculpas excesivas u otras formas de autocrítica y rendición".

**Cuando el usuario corrige o discrepa**
- Si tiene razón, cambia y agradece brevemente.
- Si no la tiene, mantén tu posición con razones; no cedas por presión (adulación, ver [`03`](./03-tono.md)).

**Cuando no puede hacerlo**
- Di qué no puede, por qué (si se puede decir) y qué sí puede hacer. Sin culpar al usuario ni sermonear.

**Transparencia de acciones (agentes con herramientas)**
- Muestra qué herramientas usó y sobre qué datos, con acceso a los detalles bajo demanda.
- Para acciones irreversibles: confirma con el detalle exacto y permite cancelar.
- Deja registro visible de lo que hizo el agente (auditoría).

**Escalar a humano**
- Ofrécelo pronto ante frustración, riesgo, o dos intentos fallidos. Entrega el contexto al humano para que el usuario no repita todo.

## Preguntas de revisión

¿El usuario sabe qué puede pedir? ¿Sabe cuándo desconfiar? ¿Puede deshacer? ¿Entiende por qué respondió así? ¿Sabe cómo darle feedback? ¿Se entera si algo cambió?

## Referencias

- Amershi, S., Weld, D., Vorvoreanu, M., Fourney, A., Nushi, B., Collisson, P., Suh, J., Iqbal, S., Bennett, P. N., Inkpen, K., Teevan, J., Kikin-Gil, R., y Horvitz, E. (2019). [Guidelines for Human-AI Interaction](https://www.microsoft.com/en-us/research/publication/guidelines-for-human-ai-interaction/). *CHI '19*, 1-13.
- Microsoft. [HAX Design Library](https://www.microsoft.com/en-us/haxtoolkit/library/): enunciados, ejemplos y patrones de cada guía.
- Anthropic. [System prompt de Claude Opus 4.7](https://platform.claude.com/docs/en/release-notes/system-prompts/claude-opus-4-7) (abril 2026): secciones sobre asesoría legal y financiera, y sobre errores y críticas.
