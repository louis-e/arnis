# One 34 s cycle: east-west roads (a) and north-south roads (b) take turns.
scoreboard players add #tl arnis 1
execute if score #tl arnis matches 680.. run scoreboard players set #tl arnis 0
execute if score #tl arnis matches 0 run function arnis:traffic/phase/a_go
# Mid-phase, both groups are set again for heads in chunks that loaded since the last switch.
execute if score #tl arnis matches 120 run function arnis:traffic/phase/a_go
execute if score #tl arnis matches 120 run function arnis:traffic/phase/b_hold
execute if score #tl arnis matches 220 run function arnis:traffic/phase/b_walk_end
execute if score #tl arnis matches 240 run function arnis:traffic/phase/a_amber
execute if score #tl arnis matches 300 run function arnis:traffic/phase/a_stop
execute if score #tl arnis matches 320 if score #red_amber arnis matches 1 run function arnis:traffic/phase/b_ready
execute if score #tl arnis matches 340 run function arnis:traffic/phase/b_go
execute if score #tl arnis matches 460 run function arnis:traffic/phase/b_go
execute if score #tl arnis matches 460 run function arnis:traffic/phase/a_hold
execute if score #tl arnis matches 560 run function arnis:traffic/phase/a_walk_end
execute if score #tl arnis matches 580 run function arnis:traffic/phase/b_amber
execute if score #tl arnis matches 640 run function arnis:traffic/phase/b_stop
execute if score #tl arnis matches 660 if score #red_amber arnis matches 1 run function arnis:traffic/phase/a_ready
