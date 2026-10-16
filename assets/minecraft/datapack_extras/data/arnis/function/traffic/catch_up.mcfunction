# Sets one lamp to what its group shows now.
scoreboard players operation @s arnis.tl = #tl_epoch arnis
execute if entity @s[tag=arnis.tl_a_g] if score #tl_a arnis matches 0 run return run function arnis:traffic/on
execute if entity @s[tag=arnis.tl_a_y] if score #tl_a arnis matches 1 run return run function arnis:traffic/on
execute if entity @s[tag=arnis.tl_a_y] if score #tl_a arnis matches 3 run return run function arnis:traffic/on
execute if entity @s[tag=arnis.tl_a_r] if score #tl_a arnis matches 2..3 run return run function arnis:traffic/on
execute if entity @s[tag=arnis.tl_b_g] if score #tl_b arnis matches 0 run return run function arnis:traffic/on
execute if entity @s[tag=arnis.tl_b_y] if score #tl_b arnis matches 1 run return run function arnis:traffic/on
execute if entity @s[tag=arnis.tl_b_y] if score #tl_b arnis matches 3 run return run function arnis:traffic/on
execute if entity @s[tag=arnis.tl_b_r] if score #tl_b arnis matches 2..3 run return run function arnis:traffic/on
execute if entity @s[tag=arnis.tl_pa_g] if score #tl_pa arnis matches 1 run return run function arnis:traffic/on
execute if entity @s[tag=arnis.tl_pa_r] if score #tl_pa arnis matches 0 run return run function arnis:traffic/on
execute if entity @s[tag=arnis.tl_pb_g] if score #tl_pb arnis matches 1 run return run function arnis:traffic/on
execute if entity @s[tag=arnis.tl_pb_r] if score #tl_pb arnis matches 0 run return run function arnis:traffic/on
function arnis:traffic/off
