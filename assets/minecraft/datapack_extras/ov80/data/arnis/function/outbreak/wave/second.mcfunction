execute if score #spawn_left arnis.ob matches 1.. if score #alive arnis.ob < #cap arnis.ob run function arnis:outbreak/wave/spawn_batch
execute if score #cleared arnis.ob matches 0 if score #spawn_left arnis.ob matches ..0 if score #alive arnis.ob matches 0 if score #boss arnis.ob matches 0 run function arnis:outbreak/wave/cleared
