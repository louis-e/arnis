# Sets one head to what its group shows now: 0 green, 1 amber, 2 red, 3 red and amber; a
# walkers' head 0 red, 1 green.
scoreboard players operation @s arnis.tl = #tl_epoch arnis
execute if entity @s[tag=arnis.tl_a] if score #tl_a arnis matches 0 run return run function arnis:traffic/car/green
execute if entity @s[tag=arnis.tl_a] if score #tl_a arnis matches 1 run return run function arnis:traffic/car/amber
execute if entity @s[tag=arnis.tl_a] if score #tl_a arnis matches 2 run return run function arnis:traffic/car/red
execute if entity @s[tag=arnis.tl_a] if score #tl_a arnis matches 3 run return run function arnis:traffic/car/red_amber
execute if entity @s[tag=arnis.tl_b] if score #tl_b arnis matches 0 run return run function arnis:traffic/car/green
execute if entity @s[tag=arnis.tl_b] if score #tl_b arnis matches 1 run return run function arnis:traffic/car/amber
execute if entity @s[tag=arnis.tl_b] if score #tl_b arnis matches 2 run return run function arnis:traffic/car/red
execute if entity @s[tag=arnis.tl_b] if score #tl_b arnis matches 3 run return run function arnis:traffic/car/red_amber
execute if entity @s[tag=arnis.tl_pa] if score #tl_pa arnis matches 0 run return run function arnis:traffic/walk/red
execute if entity @s[tag=arnis.tl_pa] if score #tl_pa arnis matches 1 run return run function arnis:traffic/walk/green
execute if entity @s[tag=arnis.tl_pb] if score #tl_pb arnis matches 0 run return run function arnis:traffic/walk/red
execute if entity @s[tag=arnis.tl_pb] if score #tl_pb arnis matches 1 run return run function arnis:traffic/walk/green
