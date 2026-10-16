execute as @e[type=block_display,tag=arnis.tl_b_y] run function arnis:traffic/off
execute as @e[type=block_display,tag=arnis.tl_b_r] run function arnis:traffic/on
scoreboard players set #tl_b arnis 2
function arnis:traffic/switched
