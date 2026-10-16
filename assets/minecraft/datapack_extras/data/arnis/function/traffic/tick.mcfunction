# One 34 s cycle: east-west roads (a) and north-south roads (b) take turns.
# A head that was out of view at a switch catches up first, so the switch below reaches it too.
execute as @e[type=block_display,tag=arnis.tl] unless score @s arnis.tl = #tl_epoch arnis run function arnis:traffic/catch_up
scoreboard players add #tl arnis 1
execute if score #tl arnis matches 680.. run scoreboard players set #tl arnis 0
execute if score #tl arnis matches 0 run function arnis:traffic/phase/a_go
execute if score #tl arnis matches 220 run function arnis:traffic/phase/b_walk_end
execute if score #tl arnis matches 240 run function arnis:traffic/phase/a_amber
execute if score #tl arnis matches 300 run function arnis:traffic/phase/a_stop
execute if score #tl arnis matches 320 if score #red_amber arnis matches 1 run function arnis:traffic/phase/b_ready
execute if score #tl arnis matches 340 run function arnis:traffic/phase/b_go
execute if score #tl arnis matches 560 run function arnis:traffic/phase/a_walk_end
execute if score #tl arnis matches 580 run function arnis:traffic/phase/b_amber
execute if score #tl arnis matches 640 run function arnis:traffic/phase/b_stop
execute if score #tl arnis matches 660 if score #red_amber arnis matches 1 run function arnis:traffic/phase/a_ready
