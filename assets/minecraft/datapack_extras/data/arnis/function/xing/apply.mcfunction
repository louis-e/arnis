# A mast or a boom takes the state in #n: 0 open, 1 the booms rising, 2 warning, 3 the
# booms coming down, 4 down.
execute if entity @s[tag=arnis.xboom] run return run function arnis:xing/boom
# A mast's lamps flash in every state but open, and its bell rings until the booms are down.
execute if score #n arnis.xing matches 0 run return run function arnis:xing/dark
tag @s add arnis.xon
tag @s remove arnis.xring
execute if score #n arnis.xing matches 2..3 if entity @s[tag=arnis.xbell] run tag @s add arnis.xring
