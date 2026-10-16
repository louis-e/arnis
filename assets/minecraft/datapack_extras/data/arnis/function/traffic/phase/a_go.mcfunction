execute as @e[type=block_display,tag=arnis.tl_a_r] run function arnis:traffic/off
execute as @e[type=block_display,tag=arnis.tl_a_y] run function arnis:traffic/off
execute as @e[type=block_display,tag=arnis.tl_a_g] run function arnis:traffic/on
execute as @e[type=block_display,tag=arnis.tl_pb_r] run function arnis:traffic/off
execute as @e[type=block_display,tag=arnis.tl_pb_g] run function arnis:traffic/on
scoreboard players set #tl_a arnis 0
scoreboard players set #tl_pb arnis 1
function arnis:traffic/switched
