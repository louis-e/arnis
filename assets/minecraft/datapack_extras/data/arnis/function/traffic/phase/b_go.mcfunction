execute as @e[type=block_display,tag=arnis.tl_b_r] run function arnis:traffic/off
execute as @e[type=block_display,tag=arnis.tl_b_y] run function arnis:traffic/off
execute as @e[type=block_display,tag=arnis.tl_b_g] run function arnis:traffic/on
execute as @e[type=block_display,tag=arnis.tl_pa_r] run function arnis:traffic/off
execute as @e[type=block_display,tag=arnis.tl_pa_g] run function arnis:traffic/on
scoreboard players set #tl_b arnis 0
scoreboard players set #tl_pa arnis 1
function arnis:traffic/switched
