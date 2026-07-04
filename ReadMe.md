Desarrollado por Claude en Claude Code.

Prompt inicial:

Vas a escribir un programa de ajedrez original tú solo.
Yo te ayudo asesorándote en lo que pueda, pero todas las decisiones técnicas las tomas tú.
El propósito no es hacer el motor más fuerte del mundo, pero sí queremos que sea fuerte.
No es necesario hacer un programa muy fuerte de primeras. Haremos iteraciones futuras para ir mejorando la fuerza. Lo que sí es imprescindible es que sea un motor completo desde el principio, que juegue ajedrez legal sin crashear ni dar errores.
Debe cumplir el protocolo UCI.
Tú decides el lenguaje de programación. A mí me gusta C/C++ porque es muy rápido, pero Python te puede proporcionar librerías que te faciliten el trabajo. También puedes utilizar lenguajes menos conocidos como rust o zig. Elige tú.
Tú decides el orden de las tareas, pero te propongo empezar por una interfaz UCI donde todas las llamadas al programa de ajedrez estén inicialmente vacías. Esto te dará un esqueleto que puedes ir rellenando por etapas.
Después de eso, te propongo que elijas la representación de la información (tablero, piezas, jugadas...). Con eso podrás programar el generador de movimientos, que es bastante independiente del resto del motor y puedes programarlo y sacarlo completamente de tu ventana de contexto, quedándote sólo con la interfaz.
La evaluación es otro módulo bastante independiente, y también puedes programarla y sacarla del contexto, dejando sólo la interfaz. Yo he escrito varios programas de ajedrez y me gusta empezar con una eval muy simple: material + movilidad, donde la movilidad la computo como el número total de jugadas legales mías menos el número total de jugadas legales del rival. Me parece un buen punto de partida, pero de nuevo tú decides cómo quieres hacerlo.
Finalmente, tenemos la búsqueda, con todas sus complejidades de profundización iterativa, PVS (o mtdf o lo que elijas), negascout, quiescent search, podas, reducciones, extensiones...
Tómate el tiempo que necesites para pensar y dime por dónde quieres empezar.

***

Hay que guiarlo, pero es bastante bueno corrigiendo errores y revisando el trabajo de otros modelos.
Las mejoras las programa sin apenas ayuda.
