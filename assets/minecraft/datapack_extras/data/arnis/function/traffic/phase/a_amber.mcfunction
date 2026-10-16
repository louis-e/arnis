execute as @e[type=block_display,tag=arnis.tl_a_g] run function arnis:traffic/off
execute as @e[type=block_display,tag=arnis.tl_a_y] run function arnis:traffic/on
scoreboard players set #tl_a arnis 1
function arnis:traffic/switched
