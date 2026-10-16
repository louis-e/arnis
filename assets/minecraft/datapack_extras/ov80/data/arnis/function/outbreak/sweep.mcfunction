execute if score #run arnis.ob matches 0 as @e[tag=arnis.ob.ent,tag=!arnis.ob.run0,tag=!arnis.ob.run1] run tag @s add arnis.ob.run0
execute if score #run arnis.ob matches 1 as @e[tag=arnis.ob.ent,tag=!arnis.ob.run0,tag=!arnis.ob.run1] run tag @s add arnis.ob.run1
execute if score #run arnis.ob matches 0 run kill @e[tag=arnis.ob.run1]
execute if score #run arnis.ob matches 1 run kill @e[tag=arnis.ob.run0]
